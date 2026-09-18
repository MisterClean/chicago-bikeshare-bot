use anyhow::{Result, ensure};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: String,
    pub station_name: String,
    pub short_name: String,
    pub total_docks: i64,
    pub docks_in_service: i64,
    pub status: String,
    pub latitude: f64,
    pub longitude: f64,
    pub is_electric: bool,
}
impl Station {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.trim().is_empty() && self.id.len() <= 256,
            "Invalid station ID"
        );
        ensure!(
            !self.station_name.trim().is_empty() && self.station_name.len() <= 512,
            "Invalid station name"
        );
        ensure!(!self.status.trim().is_empty(), "Missing station status");
        ensure!(
            self.total_docks >= 0 && self.docks_in_service >= 0,
            "Negative dock count"
        );
        ensure!(
            (41.5..=42.2).contains(&self.latitude) && (-88.0..=-87.4).contains(&self.longitude),
            "Station outside Chicago bounds"
        );
        Ok(())
    }
    pub fn display_name(&self) -> &str {
        self.station_name.trim_end_matches('*').trim_end()
    }
    pub fn state_hash(&self) -> Result<String> {
        let fields = serde_json::json!([
            self.station_name,
            self.short_name,
            self.total_docks,
            self.docks_in_service,
            self.status,
            self.latitude,
            self.longitude,
            self.is_electric
        ]);
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&fields)?)
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "station.discovered")]
    Discovered,
    #[serde(rename = "station.electrified")]
    Electrified,
}
impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "station.discovered",
            Self::Electrified => "station.electrified",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Civic,
    Nightline,
}
impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Civic => "civic",
            Self::Nightline => "nightline",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    #[serde(rename = "type")]
    pub kind: EventType,
    pub station: Station,
    pub observed_at: String,
}
#[derive(Debug)]
pub struct Delivery {
    pub id: String,
    pub record_key: String,
    pub style: Style,
    pub payload: Payload,
}

const BASE32: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";
pub fn encode_tid(mut value: u64) -> String {
    let mut output = [b'2'; 13];
    for digit in output.iter_mut().rev() {
        *digit = BASE32[(value & 31) as usize];
        value >>= 5;
    }
    output.iter().map(|&c| char::from(c)).collect()
}
pub fn decode_tid(value: &str) -> Result<u64> {
    ensure!(value.len() == 13, "Invalid record key length");
    let mut number = 0u64;
    for c in value.bytes() {
        let digit = BASE32
            .iter()
            .position(|&d| d == c)
            .ok_or_else(|| anyhow::anyhow!("Invalid TID character"))? as u64;
        number = number
            .checked_mul(32)
            .and_then(|n| n.checked_add(digit))
            .ok_or_else(|| anyhow::anyhow!("TID overflow"))?;
    }
    ensure!(number <= i64::MAX as u64, "TID exceeds 63 bits");
    Ok(number)
}
pub fn reply_key(root: &str) -> Result<String> {
    let number = decode_tid(root)?;
    // Preserve the TypeScript publisher's modulo-32 reply clock, including old queued deliveries.
    Ok(encode_tid((number & !1023) | (((number & 1023) + 1) % 32)))
}
pub fn post_text(payload: &Payload) -> String {
    let station = &payload.station;
    let title = if payload.kind == EventType::Electrified {
        "⚡ Bikeshare charging update detected"
    } else {
        "🆕 Bikeshare station newly listed"
    };
    let suffix = if payload.kind == EventType::Electrified || station.is_electric {
        "\n⚡ Charging inferred from source name."
    } else {
        ""
    };
    let ending = format!(
        "\n🚲 {} docks{suffix}\n\nUnofficial • Chicago open data\nNot live availability; check the official app.",
        station.total_docks,
    );
    let prefix = format!("{title}\n\n📍 ");
    // Unicode scalar count is a conservative bound on Bluesky's 300 graphemes.
    // Keep the source/availability notice even for an unusually long station name.
    let budget = 300usize.saturating_sub(prefix.chars().count() + ending.chars().count());
    let name = station.display_name();
    let name = if name.chars().count() > budget {
        format!(
            "{}…",
            name.chars()
                .take(budget.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        name.to_owned()
    };
    format!("{prefix}{name}{ending}")
}
