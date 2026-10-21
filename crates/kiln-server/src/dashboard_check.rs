//! Live dashboard validation for deploy and smoke checks.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{fmt, net::Ipv4Addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckError {
    EmptyBody,
    InvalidJson,
    HostCpuMissing,
    HostMemoryMissing,
    HostMemoryNotPositive,
    RunningBoxMetricsMissing,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBody => f.write_str("dashboard body is empty"),
            Self::InvalidJson => f.write_str("dashboard body is not valid JSON"),
            Self::HostCpuMissing => f.write_str("host cpu_percent is missing or null"),
            Self::HostMemoryMissing => f.write_str("host memory_bytes is missing or null"),
            Self::HostMemoryNotPositive => f.write_str("host memory_bytes must be positive"),
            Self::RunningBoxMetricsMissing => {
                f.write_str("running box metrics are missing or null")
            }
        }
    }
}

impl std::error::Error for CheckError {}

#[derive(Debug, Deserialize)]
struct WireDocument {
    host: WireHost,
    #[serde(default)]
    boxes: Vec<WireBox>,
}

#[derive(Debug, Deserialize)]
struct WireHost {
    metrics: WireMetrics,
}

#[derive(Debug, Deserialize)]
struct WireBox {
    state: String,
    metrics: WireMetrics,
}

#[derive(Debug, Deserialize, Default)]
struct WireMetrics {
    cpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
}

pub fn parse_and_validate(body: &[u8]) -> Result<(), CheckError> {
    if body.is_empty() {
        return Err(CheckError::EmptyBody);
    }
    let document: WireDocument =
        serde_json::from_slice(body).map_err(|_| CheckError::InvalidJson)?;
    validate_document(&document)
}

fn validate_document(document: &WireDocument) -> Result<(), CheckError> {
    if document.host.metrics.cpu_percent.is_none() {
        return Err(CheckError::HostCpuMissing);
    }
    let memory = document
        .host
        .metrics
        .memory_bytes
        .filter(|value| *value > 0)
        .ok_or(CheckError::HostMemoryMissing)?;
    if memory == 0 {
        return Err(CheckError::HostMemoryNotPositive);
    }
    for row in &document.boxes {
        if row.state != "running" {
            continue;
        }
        if row.metrics.cpu_percent.is_none() || row.metrics.memory_bytes.is_none() {
            return Err(CheckError::RunningBoxMetricsMissing);
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SelfCheckOptions {
    pub address: String,
    pub token_file: std::path::PathBuf,
    pub ca_file: std::path::PathBuf,
}

pub async fn run_self_check(options: SelfCheckOptions) -> Result<()> {
    let address = parse_ipv4(&options.address)?;
    let token = kiln_api::read_token(&options.token_file)
        .with_context(|| format!("read {}", options.token_file.display()))?;
    ensure!(
        token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit()),
        "admin token must be 64 hex characters"
    );
    let ca_pem = std::fs::read(&options.ca_file)
        .with_context(|| format!("read {}", options.ca_file.display()))?;
    let client = http_client(&ca_pem)?;
    let url = format!("https://{address}:8443/api/v1/dashboard");
    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .context("dashboard request failed")?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .context("dashboard response body unavailable")?;
    ensure!(
        status.is_success(),
        "dashboard request returned HTTP {status}"
    );
    parse_and_validate(&body).map_err(|error| anyhow::anyhow!("{error}"))
}

fn parse_ipv4(address: &str) -> Result<Ipv4Addr> {
    address
        .parse::<Ipv4Addr>()
        .context("address must be an IPv4 literal")
}

fn http_client(ca_pem: &[u8]) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(std::time::Duration::from_secs(30))
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem)?)
        .build()
        .context("HTTP client")
}
