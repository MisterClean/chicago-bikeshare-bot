use crate::{
    config::Config,
    domain::{Station, Style},
    http::Http,
    vector_tile::{self, Tile},
};
use anyhow::{Context, Result, ensure};
use fontdue::{Font, FontSettings};
use image::{ExtendedColorType, ImageReader, codecs::jpeg::JpegEncoder};
use prost::Message;
use serde_json::Value;
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

pub const MAX_IMAGE_BYTES: usize = 2_000_000;
pub struct PostImage {
    pub bytes: Vec<u8>,
    pub alt: String,
    pub width: u32,
    pub height: u32,
}
const FONT: &[u8] = include_bytes!("../assets/fonts/BigShouldersText-Bold.ttf");
const MAP_FONT: &[u8] = include_bytes!("../assets/fonts/BigShouldersText-Medium.ttf");

pub fn image_alt(station: &Station, description: &str) -> String {
    let query = format!(
        "SELECT id, station_name, short_name, total_docks, docks_in_service, status, latitude, longitude, location WHERE id = '{}'",
        station.id.replace('\'', "''")
    );
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    format!(
        "{description}\nUnofficial; not affiliated with or endorsed by the City of Chicago, Divvy, Lyft or CTA. Daily inventory may include inactive stations; this is not live availability. Charging is inferred from station names, not verified equipment status.\nStation ID: {}; station name: {}; short name: {}; total docks: {}; docks in service: {}; status: {}; latitude: {}; longitude: {}; location: Point [{}, {}]; charging inferred: {}\nStation data: City of Chicago / Divvy. City API record (may have changed since observation): https://data.cityofchicago.org/api/v3/views/bbyy-e7gq/query.json?query={encoded}",
        station.id,
        station.station_name,
        station.short_name,
        station.total_docks,
        station.docks_in_service,
        station.status,
        station.latitude,
        station.longitude,
        station.longitude,
        station.latitude,
        if station.is_electric { "yes" } else { "no" }
    )
}
pub fn jpeg(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let (mut low, mut high) = (1u8, 100u8);
    let mut best = None;
    while low <= high {
        let quality = low + (high - low) / 2;
        let mut output = Vec::new();
        JpegEncoder::new_with_quality(&mut output, quality).encode(
            rgb,
            width,
            height,
            ExtendedColorType::Rgb8,
        )?;
        if output.len() <= MAX_IMAGE_BYTES {
            best = Some(output);
            low = quality + 1;
        } else {
            high = quality - 1;
        }
    }
    best.context("Could not encode image within Bluesky byte limit")
}
pub fn streetview(config: &Config, station: &Station) -> Result<PostImage> {
    let mut url = url::Url::parse("https://maps.googleapis.com/maps/api/streetview")?;
    url.query_pairs_mut()
        .append_pair("size", "600x400")
        .append_pair(
            "location",
            &format!("{},{}", station.latitude, station.longitude),
        )
        .append_pair("return_error_code", "true")
        .append_pair(
            "key",
            config
                .google_key
                .as_deref()
                .context("Street View key missing")?,
        );
    let response = Http::new(config.streetview_timeout_ms).get(url.as_str(), None, 4_000_000)?;
    ensure!(
        response.status == 200,
        "Street View HTTP {}",
        response.status
    );
    let mut reader = ImageReader::new(Cursor::new(response.bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(24_000_000);
    reader.limits(limits);
    let image = reader.decode()?.into_rgb8();
    let (width, height) = image.dimensions();
    Ok(PostImage {
        bytes: jpeg(image.as_raw(), width, height)?,
        width,
        height,
        alt: image_alt(
            station,
            &format!(
                "Google Street View imagery near {}, a bikeshare station in Chicago.",
                station.display_name()
            ),
        ),
    })
}

pub fn render(
    config: &Config,
    station: &Station,
    style: Style,
    electrified: bool,
) -> Result<PostImage> {
    station.validate()?;
    for attempt in 1..=config.render_attempts {
        match render_once(config, station, style, electrified) {
            Ok(image) => return Ok(image),
            Err(error) if attempt == config.render_attempts => return Err(error),
            Err(_) => {
                eprintln!("Map render failed; retrying ({attempt})");
                std::thread::sleep(Duration::from_millis(config.render_retry_ms));
            }
        }
    }
    anyhow::bail!("Map render exhausted")
}
struct View {
    width: u32,
    height: u32,
    world: f64,
    cx: f64,
    cy: f64,
    zoom: f64,
}
impl View {
    fn new(config: &Config, station: &Station, style: Style) -> Self {
        let zoom = if style == Style::Nightline {
            config.nightline_zoom
        } else {
            config.zoom
        };
        let world = 512. * 2f64.powf(zoom) * config.pixel_ratio;
        let (cx, cy) = mercator(station.longitude, station.latitude);
        Self {
            width: (config.width as f64 * config.pixel_ratio).round() as u32,
            height: (config.height as f64 * config.pixel_ratio).round() as u32,
            world,
            cx: cx * world,
            cy: cy * world,
            zoom,
        }
    }
    fn project(&self, lon: f64, lat: f64) -> (f32, f32) {
        let (x, y) = mercator(lon, lat);
        (
            (x * self.world - self.cx + self.width as f64 / 2.) as f32,
            (y * self.world - self.cy + self.height as f64 / 2.) as f32,
        )
    }
}
fn mercator(lon: f64, lat: f64) -> (f64, f64) {
    (
        (lon + 180.) / 360.,
        (1. - (lat.to_radians().tan() + 1. / lat.to_radians().cos()).ln() / std::f64::consts::PI)
            / 2.,
    )
}
fn remaining(deadline: Instant) -> Result<Http> {
    let duration = deadline
        .checked_duration_since(Instant::now())
        .context("Map render deadline exceeded")?;
    Ok(Http::new(duration.as_millis().min(15000) as u64))
}

fn tile_template(config: &Config, deadline: Instant) -> Result<String> {
    if let Some(style) = &config.style_url {
        let json: Value = remaining(deadline)?.json(style, 2_000_000)?;
        let source = json["sources"]
            .as_object()
            .and_then(|sources| sources.values().find(|s| s["type"] == "vector"))
            .context("Style has no vector source")?;
        if let Some(template) = source["tiles"][0].as_str() {
            return Ok(template.to_owned());
        }
        if let Some(url) = source["url"].as_str() {
            let url = url::Url::parse(style)?.join(url)?;
            let metadata: Value = remaining(deadline)?.json(url.as_str(), 1_000_000)?;
            return metadata["tiles"][0]
                .as_str()
                .map(str::to_owned)
                .context("TileJSON has no tile template");
        }
        anyhow::bail!("Style requires a Protomaps-compatible HTTP MVT source");
    }
    let key = config
        .protomaps_key
        .as_deref()
        .context("Protomaps key required for rendering")?;
    Ok(format!(
        "https://api.protomaps.com/tiles/v4/{{z}}/{{x}}/{{y}}.mvt?key={}",
        url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>()
    ))
}
#[derive(Clone)]
struct Label {
    text: String,
    x: f32,
    y: f32,
    road: bool,
}
fn render_once(
    config: &Config,
    station: &Station,
    style: Style,
    electrified: bool,
) -> Result<PostImage> {
    let deadline = Instant::now() + Duration::from_millis(config.render_timeout_ms);
    let view = View::new(config, station, style);
    let mut pixmap = Pixmap::new(view.width, view.height).context("Allocate map canvas")?;
    pixmap.fill(Color::from_rgba8(235, 240, 239, 255));
    let template = tile_template(config, deadline)?;
    let z = view.zoom.floor().min(15.) as u32;
    let count = 2f64.powi(z as i32);
    let tile_size = view.world / count;
    let min_x = ((view.cx - view.width as f64 / 2.) / tile_size).floor() as i32;
    let max_x = ((view.cx + view.width as f64 / 2.) / tile_size).floor() as i32;
    let min_y = ((view.cy - view.height as f64 / 2.) / tile_size).floor() as i32;
    let max_y = ((view.cy + view.height as f64 / 2.) / tile_size).floor() as i32;
    ensure!(
        (max_x - min_x + 1) * (max_y - min_y + 1) <= 36,
        "Map requires too many tiles"
    );
    let mut labels = Vec::new();
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let url = template
                .replace("{z}", &z.to_string())
                .replace("{x}", &x.to_string())
                .replace("{y}", &y.to_string());
            let response = remaining(deadline)?.get(&url, None, 4_000_000)?;
            ensure!(response.status == 200, "Map tile HTTP {}", response.status);
            let tile = Tile::decode(response.bytes.as_slice()).context("Decode map tile")?;
            drop(response);
            let origin = (
                x as f64 * tile_size - view.cx + view.width as f64 / 2.,
                y as f64 * tile_size - view.cy + view.height as f64 / 2.,
            );
            draw_tile(&mut pixmap, &tile, origin, tile_size, &mut labels)?;
        }
    }
    draw_transit(&mut pixmap, &view, deadline, &mut labels)?;
    let map_font = Font::from_bytes(MAP_FONT, FontSettings::default())
        .map_err(|_| anyhow::anyhow!("Invalid map font"))?;
    draw_labels(&mut pixmap, &map_font, labels, view.width as f32 / 1080.);
    drop(map_font);
    let font = Font::from_bytes(FONT, FontSettings::default())
        .map_err(|_| anyhow::anyhow!("Invalid bundled font"))?;
    draw_card(&mut pixmap, &font, station, style, electrified);
    drop(font);
    let mut rgb = Vec::with_capacity(view.width as usize * view.height as usize * 3);
    for pixel in pixmap.data().chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    drop(pixmap);
    let bytes = jpeg(&rgb, view.width, view.height)?;
    Ok(PostImage {
        bytes,
        width: view.width,
        height: view.height,
        alt: image_alt(
            station,
            &format!(
                "Announcement map centered on {}, showing the bikeshare station, nearby streets and CTA transit.",
                station.display_name()
            ),
        ),
    })
}
fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint
}
fn hex(value: u32) -> Color {
    Color::from_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255)
}
fn draw_tile(
    canvas: &mut Pixmap,
    tile: &Tile,
    origin: (f64, f64),
    size: f64,
    labels: &mut Vec<Label>,
) -> Result<()> {
    let mut mask =
        tiny_skia::Mask::new(canvas.width(), canvas.height()).context("Allocate tile mask")?;
    if let Some(rect) = Rect::from_xywh(origin.0 as f32, origin.1 as f32, size as f32, size as f32)
    {
        let p = PathBuilder::from_rect(rect);
        mask.fill_path(&p, FillRule::Winding, false, Transform::identity());
    }
    for name in ["earth", "landuse", "water", "buildings", "roads", "pois"] {
        for layer in tile.layers.iter().filter(|l| l.name == name) {
            let extent = layer.extent.unwrap_or(4096);
            ensure!(extent > 0, "Invalid tile extent");
            let scale = size as f32 / extent as f32;
            let transform =
                Transform::from_row(scale, 0., 0., scale, origin.0 as f32, origin.1 as f32);
            for feature in &layer.features {
                let kind = layer.property(feature, "kind").unwrap_or("");
                let (path, points) = vector_tile::geometry(feature)?;
                if let Some(path) = path {
                    if feature.kind == Some(3) {
                        let color = match name {
                            "earth" => 0xebf0ef,
                            "water" => 0x9dd5ed,
                            "buildings" => 0xd9dfe1,
                            "landuse"
                                if matches!(
                                    kind,
                                    "park"
                                        | "garden"
                                        | "wood"
                                        | "forest"
                                        | "grass"
                                        | "cemetery"
                                        | "pitch"
                                        | "recreation_ground"
                                ) =>
                            {
                                0xcde1c8
                            }
                            "landuse" => 0xe5e9e4,
                            _ => continue,
                        };
                        canvas.fill_path(
                            &path,
                            &paint(hex(color)),
                            FillRule::EvenOdd,
                            transform,
                            Some(&mask),
                        );
                    } else if name == "roads" && feature.kind == Some(2) {
                        let width = if matches!(kind, "highway" | "major_road") {
                            8.
                        } else if kind == "rail" {
                            3.
                        } else {
                            5.
                        };
                        for (color, w) in [
                            (0xaebbc8, width + 2.),
                            (if kind == "rail" { 0x657382 } else { 0xfcfdfe }, width),
                        ] {
                            let stroke = Stroke {
                                width: w / scale,
                                line_cap: tiny_skia::LineCap::Round,
                                line_join: tiny_skia::LineJoin::Round,
                                ..Stroke::default()
                            };
                            canvas.stroke_path(
                                &path,
                                &paint(hex(color)),
                                &stroke,
                                transform,
                                Some(&mask),
                            );
                        }
                    }
                }
                if labels.len() < 1500
                    && matches!(name, "roads" | "pois")
                    && let Some(text) = layer
                        .property(feature, "name:en")
                        .or_else(|| layer.property(feature, "name"))
                    && text.len() < 160
                    && let Some(&(px, py)) = points.get(points.len() / 2)
                    && (0.0..extent as f32).contains(&px)
                    && (0.0..extent as f32).contains(&py)
                {
                    labels.push(Label {
                        text: text.into(),
                        x: origin.0 as f32 + px * scale,
                        y: origin.1 as f32 + py * scale,
                        road: name == "roads",
                    });
                }
            }
        }
    }
    Ok(())
}
fn draw_transit(
    canvas: &mut Pixmap,
    view: &View,
    deadline: Instant,
    labels: &mut Vec<Label>,
) -> Result<()> {
    let half_x = view.width as f64 / 2. / view.world;
    let half_y = view.height as f64 / 2. / view.world;
    let lon = |x: f64| x * 360. - 180.;
    let lat = |y: f64| {
        (std::f64::consts::PI * (1. - 2. * y))
            .sinh()
            .atan()
            .to_degrees()
    };
    let west = lon(view.cx / view.world - half_x);
    let east = lon(view.cx / view.world + half_x);
    let north = lat(view.cy / view.world - half_y);
    let south = lat(view.cy / view.world + half_y);
    let polygon = format!(
        "POLYGON (({west} {south}, {east} {south}, {east} {north}, {west} {north}, {west} {south}))"
    );
    for (dataset, base_color, width) in [
        ("6uva-a5ei", 0x73acc2, 2.7),
        ("xbyr-jnvx", 0x59636f, 5.),
        ("3tzw-cg4m", 0xffffff, 5.),
    ] {
        let mut url = url::Url::parse(&format!(
            "https://data.cityofchicago.org/resource/{dataset}.geojson"
        ))?;
        url.query_pairs_mut()
            .append_pair("$limit", "5000")
            .append_pair("$where", &format!("intersects(the_geom, '{polygon}')"));
        let json: Value = remaining(deadline)?
            .json(url.as_str(), 4_000_000)
            .context("Fetch local CTA overlay")?;
        let features = json["features"].as_array().context("Invalid CTA GeoJSON")?;
        ensure!(features.len() < 5000, "CTA overlay reached query limit");
        for feature in features {
            let color = if dataset == "xbyr-jnvx" {
                let lines = feature["properties"]["lines"].as_str().unwrap_or("");
                [
                    ("Red", 0xc60c30),
                    ("Blue", 0x00a1de),
                    ("Brown", 0x62361b),
                    ("Green", 0x009b3a),
                    ("Orange", 0xf9461c),
                    ("Pink", 0xe27ea6),
                    ("Purple", 0x522398),
                    ("Yellow", 0xf9e300),
                ]
                .into_iter()
                .find(|(name, _)| lines.contains(name))
                .map_or(base_color, |(_, color)| color)
            } else {
                base_color
            };
            geo_geometry(canvas, view, &feature["geometry"], hex(color), width)?;
            if dataset == "3tzw-cg4m"
                && let Some(name) = feature["properties"]["longname"].as_str()
            {
                let (x, y) = geo_point(view, &feature["geometry"]["coordinates"])?;
                labels.insert(
                    0,
                    Label {
                        text: name.to_owned(),
                        x,
                        y: y + 22.0,
                        road: true,
                    },
                );
            }
        }
    }
    Ok(())
}
fn geo_geometry(
    canvas: &mut Pixmap,
    view: &View,
    geometry: &Value,
    color: Color,
    width: f32,
) -> Result<()> {
    let coordinates = &geometry["coordinates"];
    match geometry["type"].as_str() {
        Some("LineString") => geo_line(canvas, view, coordinates, color, width)?,
        Some("MultiLineString") => {
            for line in coordinates.as_array().context("Invalid multiline")? {
                geo_line(canvas, view, line, color, width)?;
            }
        }
        Some("Point") => {
            let (x, y) = geo_point(view, coordinates)?;
            circle(canvas, x, y, 6., hex(0x17202a));
            circle(canvas, x, y, 4., hex(0xffffff));
        }
        _ => {}
    }
    Ok(())
}
fn geo_point(view: &View, point: &Value) -> Result<(f32, f32)> {
    let lon = point[0].as_f64().context("Invalid longitude")?;
    let lat = point[1].as_f64().context("Invalid latitude")?;
    ensure!(
        lon.is_finite() && lat.is_finite() && lat.abs() < 86.,
        "Invalid geographic point"
    );
    Ok(view.project(lon, lat))
}
fn geo_line(
    canvas: &mut Pixmap,
    view: &View,
    points: &Value,
    color: Color,
    width: f32,
) -> Result<()> {
    let mut path = PathBuilder::new();
    for (index, point) in points
        .as_array()
        .context("Invalid line")?
        .iter()
        .enumerate()
    {
        let (x, y) = geo_point(view, point)?;
        if index == 0 {
            path.move_to(x, y);
        } else {
            path.line_to(x, y);
        }
    }
    if let Some(path) = path.finish() {
        canvas.stroke_path(
            &path,
            &paint(color),
            &Stroke {
                width,
                line_cap: tiny_skia::LineCap::Round,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
    Ok(())
}
fn circle(canvas: &mut Pixmap, x: f32, y: f32, radius: f32, color: Color) {
    if let Some(path) = PathBuilder::from_circle(x, y, radius) {
        canvas.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}
fn rect(canvas: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if let Some(rect) = Rect::from_xywh(x, y, w, h) {
        canvas.fill_rect(rect, &paint(color), Transform::identity(), None);
    }
}
fn text_width(font: &Font, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|c| font.metrics(c, size).advance_width)
        .sum()
}
fn text(
    canvas: &mut Pixmap,
    font: &Font,
    label: &str,
    x: f32,
    baseline: f32,
    size: f32,
    color: Color,
) {
    let (mut pen, width, height) = (x, canvas.width() as i32, canvas.height() as i32);
    for ch in label.chars() {
        let (metrics, bitmap) = font.rasterize(ch, size);
        let top = baseline as i32 - metrics.height as i32 - metrics.ymin;
        let left = pen as i32 + metrics.xmin;
        for gy in 0..metrics.height {
            let py = top + gy as i32;
            if py < 0 || py >= height {
                continue;
            }
            for gx in 0..metrics.width {
                let px = left + gx as i32;
                if px < 0 || px >= width {
                    continue;
                }
                let alpha = bitmap[gy * metrics.width + gx] as f32 / 255. * color.alpha();
                if alpha == 0. {
                    continue;
                }
                let offset = (py as usize * width as usize + px as usize) * 4;
                let data = &mut canvas.data_mut()[offset..offset + 4];
                for (i, value) in [color.red(), color.green(), color.blue()]
                    .into_iter()
                    .enumerate()
                {
                    data[i] = (value * 255. * alpha + data[i] as f32 * (1. - alpha)).round() as u8;
                }
                data[3] = 255;
            }
        }
        pen += metrics.advance_width;
    }
}
fn draw_labels(canvas: &mut Pixmap, font: &Font, mut labels: Vec<Label>, scale: f32) {
    labels.sort_by_key(|l| !l.road);
    let mut occupied: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut names = std::collections::HashSet::new();
    for label in labels {
        let size = if label.road { 19. } else { 17. } * scale;
        let width = text_width(font, &label.text, size);
        let x = label.x - width / 2.;
        let y = label.y;
        if x < 12.
            || x + width > canvas.width() as f32 - 12.
            || y < 165. * scale
            || y > canvas.height() as f32 * 0.72
            || ((label.x - canvas.width() as f32 / 2.).abs() < 65. * scale
                && (y - canvas.height() as f32 / 2.).abs() < 65. * scale)
        {
            continue;
        }
        let bounds = (x - 5., y - size - 4., x + width + 5., y + 7.);
        if names.contains(&label.text)
            || occupied
                .iter()
                .any(|b| bounds.0 < b.2 && bounds.2 > b.0 && bounds.1 < b.3 && bounds.3 > b.1)
        {
            continue;
        }
        for (dx, dy) in [(-1.5, 0.), (1.5, 0.), (0., -1.5), (0., 1.5)] {
            text(
                canvas,
                font,
                &label.text,
                x + dx,
                y + dy,
                size,
                hex(0xffffff),
            );
        }
        text(
            canvas,
            font,
            &label.text,
            x,
            y,
            size,
            hex(if label.road { 0x29445f } else { 0x65705f }),
        );
        occupied.push(bounds);
        names.insert(label.text);
    }
}
fn star(canvas: &mut Pixmap, x: f32, y: f32, radius: f32) {
    let mut path = PathBuilder::new();
    for i in 0..12 {
        let a = i as f32 * std::f32::consts::PI / 6. - std::f32::consts::PI / 2.;
        let r = if i % 2 == 0 { radius } else { radius * 0.42 };
        let (px, py) = (x + a.cos() * r, y + a.sin() * r);
        if i == 0 {
            path.move_to(px, py);
        } else {
            path.line_to(px, py);
        }
    }
    path.close();
    if let Some(path) = path.finish() {
        canvas.fill_path(
            &path,
            &paint(hex(0xe4002b)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}
fn draw_card(canvas: &mut Pixmap, font: &Font, station: &Station, style: Style, electrified: bool) {
    let w = canvas.width() as f32;
    let h = canvas.height() as f32;
    let scale = (w / 1080.).min(h / 1350.);
    let night = style == Style::Nightline;
    let background = if night { [1., 2., 5.] } else { [5., 26., 55.] };
    for (y, row) in canvas
        .data_mut()
        .chunks_exact_mut(w as usize * 4)
        .enumerate()
    {
        let alpha = ((y as f32 / h - 0.52) / 0.36).clamp(0., 1.);
        for pixel in row.chunks_exact_mut(4) {
            for i in 0..3 {
                pixel[i] = (pixel[i] as f32 * (1. - alpha) + background[i] * alpha) as u8;
            }
        }
    }
    if !night {
        for i in 0..4 {
            star(
                canvas,
                w - 65. * scale - i as f32 * 51. * scale,
                67. * scale,
                19. * scale,
            );
        }
        rect(canvas, 0., h - 18. * scale, w, 6. * scale, hex(0x51c2f0));
        rect(canvas, 0., h - 12. * scale, w, 12. * scale, hex(0xe4002b));
    }
    circle(
        canvas,
        w / 2.,
        h / 2.,
        41. * scale,
        Color::from_rgba8(255, 255, 255, 210),
    );
    star(canvas, w / 2., h / 2., 37. * scale);
    let eyebrow = if electrified {
        "BIKESHARE STATION ELECTRIFIED"
    } else {
        "NEW BIKESHARE STATION"
    };
    let status = if electrified {
        "CHARGED"
    } else if night {
        "DEPLOYED"
    } else {
        "OPEN"
    };
    let margin = 48. * scale;
    let baseline = h - 420. * scale;
    let label_size = 32. * scale;
    let label_w = text_width(font, eyebrow, label_size);
    let label_x = if night {
        (w - label_w) / 2.
    } else {
        margin + 66. * scale
    };
    if night {
        rect(
            canvas,
            label_x - 14. * scale,
            baseline - label_size,
            label_w + 28. * scale,
            42. * scale,
            hex(0xe4002b),
        );
    } else {
        rect(
            canvas,
            margin,
            baseline - 12. * scale,
            50. * scale,
            8. * scale,
            hex(0xe4002b),
        );
    }
    text(
        canvas,
        font,
        eyebrow,
        label_x,
        baseline,
        label_size,
        hex(if night { 0xffffff } else { 0x51c2f0 }),
    );
    let mut size = if night { 235. } else { 265. } * scale;
    size = size.min((w - margin * 2.) / text_width(font, status, 1.));
    let status_w = text_width(font, status, size);
    let sx = if night { (w - status_w) / 2. } else { margin };
    let sy = h - 170. * scale;
    if night {
        text(
            canvas,
            font,
            status,
            sx + 5. * scale,
            sy + 5. * scale,
            size,
            hex(0x51c2f0),
        );
    }
    text(canvas, font, status, sx, sy, size, hex(0xffffff));
    let title = station.display_name().to_uppercase();
    let title_size = (72. * scale).min((w - 2. * margin) / text_width(font, &title, 1.));
    let title_x = if night {
        (w - text_width(font, &title, title_size)) / 2.
    } else {
        margin
    };
    text(
        canvas,
        font,
        &title,
        title_x,
        h - 104. * scale,
        title_size,
        hex(0xffffff),
    );
    let details = format!(
        "{} DOCKS{}",
        station.total_docks,
        if station.is_electric {
            "   /   ELECTRIFIED"
        } else {
            ""
        }
    );
    let detail_size = 29. * scale;
    let dx = if night {
        (w - text_width(font, &details, detail_size)) / 2.
    } else {
        margin
    };
    text(
        canvas,
        font,
        &details,
        dx,
        h - 58. * scale,
        detail_size,
        hex(if night { 0xb7bec8 } else { 0xffffff }),
    );
    let attribution =
        "Data: City of Chicago / CTA · © Protomaps · © OpenStreetMap contributors · Unofficial";
    let size = (18. * scale).min((w - 36. * scale) / text_width(font, attribution, 1.));
    let x = (w - text_width(font, attribution, size)) / 2.;
    text(
        canvas,
        font,
        attribution,
        x,
        h - if night { 14. } else { 28. } * scale,
        size,
        hex(0xc3c9d2),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn native_cards_support_both_styles_and_electrification() {
        let font = Font::from_bytes(FONT, FontSettings::default()).unwrap();
        let mut station = crate::tests::station("1");
        station.station_name = "Dr Martin Luther King Jr Dr & Oakwood Blvd*".into();
        station.is_electric = true;
        for style in [Style::Civic, Style::Nightline] {
            let mut canvas = Pixmap::new(1080, 1350).unwrap();
            canvas.fill(hex(0xebf0ef));
            draw_card(&mut canvas, &font, &station, style, true);
            assert_ne!(canvas.pixel(540, 675).unwrap(), canvas.pixel(1, 1).unwrap());
            assert!(canvas.pixel(10, 1300).unwrap().red() < 10);
        }
    }
    #[test]
    fn vector_geometry_projects_a_real_polygon_without_browser() {
        let feature = vector_tile::Feature {
            tags: vec![],
            kind: Some(3),
            geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
        };
        let tile = Tile {
            layers: vec![vector_tile::Layer {
                name: "water".into(),
                features: vec![feature],
                keys: vec![],
                values: vec![],
                extent: Some(4096),
            }],
        };
        let mut canvas = Pixmap::new(256, 256).unwrap();
        canvas.fill(hex(0xffffff));
        draw_tile(&mut canvas, &tile, (0., 0.), 256., &mut Vec::new()).unwrap();
        assert_eq!(canvas.pixel(128, 128).unwrap().blue(), 237);
    }
}
