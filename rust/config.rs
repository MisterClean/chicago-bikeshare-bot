use anyhow::{Context, Result, bail, ensure};
use std::{collections::HashMap, path::PathBuf};

pub struct Config {
    pub db_path: PathBuf,
    pub soda_url: String,
    pub soda_timeout_ms: u64,
    pub soda_retries: u32,
    pub soda_min: usize,
    pub publish: bool,
    pub post_limit: usize,
    pub bluesky_url: String,
    pub identifier: Option<String>,
    pub password: Option<String>,
    pub protomaps_key: Option<String>,
    pub style_url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub pixel_ratio: f64,
    pub zoom: f64,
    pub nightline_zoom: f64,
    pub render_timeout_ms: u64,
    pub render_attempts: u32,
    pub render_retry_ms: u64,
    pub streetview: bool,
    pub google_key: Option<String>,
    pub streetview_timeout_ms: u64,
    pub output: PathBuf,
}
impl Config {
    pub fn load() -> Result<Self> {
        Self::from_map(&std::env::vars().collect())
    }
    pub fn from_map(env: &HashMap<String, String>) -> Result<Self> {
        let text = |key: &str, fallback: &str| {
            env.get(key).cloned().unwrap_or_else(|| fallback.to_owned())
        };
        let optional = |key: &str| env.get(key).filter(|v| !v.is_empty()).cloned();
        let boolean = |key: &str| -> Result<bool> {
            match text(key, "false").as_str() {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                _ => bail!("{key} must be true, false, 1 or 0"),
            }
        };
        let number = |key: &str, fallback: f64, min: f64, max: f64, integer: bool| -> Result<f64> {
            let value: f64 = text(key, &fallback.to_string())
                .parse()
                .with_context(|| format!("Invalid {key}"))?;
            ensure!(
                value.is_finite()
                    && (min..=max).contains(&value)
                    && (!integer || value.fract() == 0.0),
                "Invalid {key}: outside supported range"
            );
            Ok(value)
        };
        let config = Self {
            db_path: text("DB_PATH", "data/chicago-bikeshare-bot.sqlite3").into(),
            soda_url: text(
                "SODA_URL",
                "https://data.cityofchicago.org/resource/bbyy-e7gq.json",
            ),
            soda_timeout_ms: number("SODA_TIMEOUT_MS", 30000., 1., 300000., true)? as u64,
            soda_retries: number("SODA_MAX_RETRIES", 3., 0., 10., true)? as u32,
            soda_min: number("SODA_MIN_STATIONS", 500., 1., 49999., true)? as usize,
            publish: boolean("PUBLISH_ENABLED")?,
            post_limit: number("POST_LIMIT", 10., 1., 100., true)? as usize,
            bluesky_url: text("BLUESKY_SERVICE_URL", "https://bsky.social"),
            identifier: optional("BLUESKY_IDENTIFIER"),
            password: optional("BLUESKY_APP_PASSWORD"),
            protomaps_key: optional("PROTOMAPS_KEY"),
            style_url: optional("PROTOMAPS_STYLE_URL"),
            width: number("MAP_WIDTH", 1080., 600., 2400., true)? as u32,
            height: number("MAP_HEIGHT", 1350., 600., 2400., true)? as u32,
            pixel_ratio: number("MAP_PIXEL_RATIO", 1., 1., 3., false)?,
            zoom: number("MAP_ZOOM", 17., 12., 19., false)?,
            nightline_zoom: number("MAP_NIGHTLINE_ZOOM", 17.5, 12., 19., false)?,
            render_timeout_ms: number("MAP_RENDER_TIMEOUT_MS", 75000., 10000., 180000., true)?
                as u64,
            render_attempts: number("MAP_RENDER_MAX_ATTEMPTS", 2., 1., 5., true)? as u32,
            render_retry_ms: number("MAP_RENDER_RETRY_DELAY_MS", 2000., 0., 30000., true)? as u64,
            streetview: boolean("STREETVIEW_ENABLED")?,
            google_key: optional("GOOGLE_MAPS_API_KEY"),
            streetview_timeout_ms: number("STREETVIEW_TIMEOUT_MS", 10000., 1., 120000., true)?
                as u64,
            output: text("OUTPUT_DIR", "output").into(),
        };
        ensure!(
            !config.db_path.as_os_str().is_empty(),
            "DB_PATH must not be empty"
        );
        for value in [&config.soda_url, &config.bluesky_url]
            .into_iter()
            .chain(config.style_url.iter())
        {
            let url = url::Url::parse(value).context("Invalid service URL")?;
            ensure!(
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
                "Expected an HTTP(S) service URL"
            );
        }
        let auth_url = url::Url::parse(&config.bluesky_url)?;
        ensure!(
            auth_url.scheme() == "https"
                || matches!(auth_url.host_str(), Some("localhost" | "127.0.0.1")),
            "Bluesky login requires HTTPS outside loopback tests"
        );
        if config.publish {
            ensure!(
                config.identifier.is_some() && config.password.is_some(),
                "Bluesky credentials required when publishing"
            );
            ensure!(
                config.protomaps_key.is_some() || config.style_url.is_some(),
                "Protomaps configuration required when publishing"
            );
        }
        ensure!(
            !config.streetview || config.google_key.is_some(),
            "GOOGLE_MAPS_API_KEY required for Street View"
        );
        // Bound the two full-frame image buffers even for inherited browser configurations.
        ensure!(
            (config.width as f64 * config.height as f64 * config.pixel_ratio.powi(2)) <= 6_000_000.,
            "Map exceeds six megapixel memory budget"
        );
        Ok(config)
    }
    pub fn redact(&self, message: &str) -> String {
        let mut clean = message.to_owned();
        for secret in [
            &self.password,
            &self.protomaps_key,
            &self.google_key,
            &self.style_url,
        ]
        .into_iter()
        .flatten()
        {
            clean = clean.replace(secret, "[REDACTED]");
            clean = clean.replace(
                &url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>(),
                "[REDACTED]",
            );
        }
        clean.chars().take(2000).collect()
    }
}
