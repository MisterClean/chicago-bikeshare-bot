use crate::{config::Config, domain::Station, http::Http};
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};

#[derive(Deserialize)]
struct Record {
    id: String,
    station_name: String,
    #[serde(default)]
    short_name: String,
    #[serde(deserialize_with = "number")]
    total_docks: f64,
    #[serde(deserialize_with = "number")]
    docks_in_service: f64,
    status: String,
    #[serde(deserialize_with = "number")]
    latitude: f64,
    #[serde(deserialize_with = "number")]
    longitude: f64,
}
fn number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        String(String),
        Number(f64),
    }
    match Number::deserialize(deserializer)? {
        Number::Number(n) => Ok(n),
        Number::String(s) => s.parse().map_err(serde::de::Error::custom),
    }
}
pub fn parse(bytes: &[u8], minimum: usize) -> Result<Vec<Station>> {
    let records: Vec<Record> = serde_json::from_slice(bytes)?;
    ensure!(
        !records.is_empty() && records.len() >= minimum,
        "Station feed below minimum ({}/{minimum})",
        records.len()
    );
    ensure!(
        records.len() < 50000,
        "Station feed reached query limit; refusing possibly truncated snapshot"
    );
    let mut ids = HashSet::with_capacity(records.len());
    records
        .into_iter()
        .map(|r| {
            ensure!(
                r.total_docks.fract() == 0.
                    && r.docks_in_service.fract() == 0.
                    && (0.0..=i32::MAX as f64).contains(&r.total_docks)
                    && (0.0..=i32::MAX as f64).contains(&r.docks_in_service),
                "Non-integral dock count"
            );
            let name = r.station_name.trim().to_owned();
            let short = r.short_name.trim().to_owned();
            let station = Station {
                id: r.id,
                is_electric: name.ends_with('*') || short.to_ascii_lowercase().contains("charging"),
                station_name: name,
                short_name: short,
                total_docks: r.total_docks as i64,
                docks_in_service: r.docks_in_service as i64,
                status: r.status,
                latitude: r.latitude,
                longitude: r.longitude,
            };
            station.validate()?;
            ensure!(ids.insert(station.id.clone()), "Duplicate station ID");
            Ok(station)
        })
        .collect()
}
pub fn fetch(config: &Config) -> Result<Vec<Station>> {
    let http = Http::new(config.soda_timeout_ms);
    let mut url = url::Url::parse(&config.soda_url)?;
    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "$limit" && key != "$order")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(retained)
        .append_pair("$limit", "50000")
        .append_pair("$order", "id");
    for attempt in 0..=config.soda_retries {
        let result = (|| {
            let response = http.get(url.as_str(), None, 16_000_000)?;
            ensure!(
                response.status == 200,
                "Station feed HTTP {}",
                response.status
            );
            parse(&response.bytes, config.soda_min)
        })();
        match result {
            Ok(stations) => return Ok(stations),
            Err(error) if attempt == config.soda_retries => return Err(error),
            Err(_) => {
                eprintln!("Station fetch failed; retry {}", attempt + 1);
                std::thread::sleep(Duration::from_secs(1 << attempt));
            }
        }
    }
    anyhow::bail!("Station fetch exhausted")
}
