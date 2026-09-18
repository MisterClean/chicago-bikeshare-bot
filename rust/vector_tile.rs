//! The subset of the Mapbox Vector Tile v2 protobuf schema used by Protomaps.
use anyhow::{Result, ensure};
use prost::Message;
use tiny_skia::PathBuilder;

#[derive(Clone, PartialEq, Message)]
pub struct Tile {
    #[prost(message, repeated, tag = "3")]
    pub layers: Vec<Layer>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Layer {
    #[prost(string, required, tag = "1")]
    pub name: String,
    #[prost(message, repeated, tag = "2")]
    pub features: Vec<Feature>,
    #[prost(string, repeated, tag = "3")]
    pub keys: Vec<String>,
    #[prost(message, repeated, tag = "4")]
    pub values: Vec<Value>,
    #[prost(uint32, optional, tag = "5", default = "4096")]
    pub extent: Option<u32>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Feature {
    #[prost(uint32, repeated, packed = "true", tag = "2")]
    pub tags: Vec<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub kind: Option<u32>,
    #[prost(uint32, repeated, packed = "true", tag = "4")]
    pub geometry: Vec<u32>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Value {
    #[prost(string, optional, tag = "1")]
    pub string: Option<String>,
}
impl Layer {
    pub fn property<'a>(&'a self, feature: &Feature, name: &str) -> Option<&'a str> {
        feature.tags.chunks_exact(2).find_map(|tag| {
            if self.keys.get(tag[0] as usize)?.as_str() != name {
                return None;
            }
            self.values.get(tag[1] as usize)?.string.as_deref()
        })
    }
}
pub type Geometry = (Option<tiny_skia::Path>, Vec<(f32, f32)>);

pub fn geometry(feature: &Feature) -> Result<Geometry> {
    ensure!(
        feature.geometry.len() <= 250_000,
        "Tile feature geometry exceeds budget"
    );
    let mut builder = PathBuilder::new();
    let mut points = Vec::new();
    let mut cursor = 0;
    let (mut x, mut y) = (0i64, 0i64);
    let mut has_start = false;
    while cursor < feature.geometry.len() {
        let command = feature.geometry[cursor];
        cursor += 1;
        let count = command >> 3;
        ensure!(
            count > 0 && count <= 125_000,
            "Invalid geometry command count"
        );
        match command & 7 {
            1 | 2 => {
                let moving = command & 7 == 1;
                ensure!(
                    cursor + count as usize * 2 <= feature.geometry.len(),
                    "Truncated tile geometry"
                );
                for _ in 0..count {
                    let dx = feature.geometry[cursor];
                    let dy = feature.geometry[cursor + 1];
                    cursor += 2;
                    x += ((dx >> 1) as i64) ^ -((dx & 1) as i64);
                    y += ((dy >> 1) as i64) ^ -((dy & 1) as i64);
                    ensure!(
                        x.abs() < 10_000_000 && y.abs() < 10_000_000,
                        "Invalid geometry coordinate"
                    );
                    if moving {
                        builder.move_to(x as f32, y as f32);
                        has_start = true;
                    } else {
                        ensure!(has_start, "Line without origin");
                        builder.line_to(x as f32, y as f32);
                    }
                    points.push((x as f32, y as f32));
                }
            }
            7 => {
                ensure!(count == 1 && has_start, "Invalid polygon closure");
                builder.close();
            }
            _ => anyhow::bail!("Unknown tile geometry command"),
        }
    }
    Ok((builder.finish(), points))
}
