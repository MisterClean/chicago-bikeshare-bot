use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{io::Read, time::Duration};

pub struct Http {
    agent: ureq::Agent,
}
pub struct Response {
    pub status: u16,
    pub bytes: Vec<u8>,
}
impl Http {
    pub fn new(timeout_ms: u64) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(timeout_ms)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Self {
            agent: config.into(),
        }
    }
    fn read(mut response: ureq::http::Response<ureq::Body>, limit: u64) -> Result<Response> {
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .context("Read HTTP response")?;
        ensure!(
            bytes.len() as u64 <= limit,
            "HTTP response exceeds {limit} byte limit"
        );
        Ok(Response { status, bytes })
    }
    pub fn get(&self, url: &str, token: Option<&str>, limit: u64) -> Result<Response> {
        let mut request = self
            .agent
            .get(url)
            .header("User-Agent", "chicago-bikeshare-bot/3.0");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        Self::read(
            request
                .call()
                .map_err(|_| anyhow::anyhow!("HTTP GET transport failure"))?,
            limit,
        )
    }
    pub fn json<T: DeserializeOwned>(&self, url: &str, limit: u64) -> Result<T> {
        let response = self.get(url, None, limit)?;
        ensure!(
            response.status == 200,
            "HTTP GET returned {}",
            response.status
        );
        Ok(serde_json::from_slice(&response.bytes)?)
    }
    pub fn post(
        &self,
        url: &str,
        token: Option<&str>,
        body: &[u8],
        mime: &str,
    ) -> Result<Response> {
        let mut request = self
            .agent
            .post(url)
            .header("User-Agent", "chicago-bikeshare-bot/3.0")
            .header("Content-Type", mime);
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        Self::read(
            request
                .send(body)
                .map_err(|_| anyhow::anyhow!("HTTP POST transport failure"))?,
            2_000_000,
        )
    }
    pub fn post_json(&self, url: &str, token: Option<&str>, body: &Value) -> Result<Response> {
        self.post(url, token, &serde_json::to_vec(body)?, "application/json")
    }
}
