//! Live dashboard validation for deploy and smoke checks.

use anyhow::{Context, Result, ensure};
use std::{fmt, net::Ipv4Addr};

use crate::dashboard::Document;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckError {
    EmptyBody,
    InvalidJson,
    HostCpuMissing,
    HostMemoryMissing,
    HostMemoryNotPositive,
    HostDiskMissing,
    HostDiskTotalMissing,
    RunningLaunchTimingMissing,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBody => f.write_str("dashboard body is empty"),
            Self::InvalidJson => f.write_str("dashboard body is not valid JSON"),
            Self::HostCpuMissing => f.write_str("host cpu_percent is missing or null"),
            Self::HostMemoryMissing => f.write_str("host memory_bytes is missing or null"),
            Self::HostMemoryNotPositive => f.write_str("host memory_bytes must be positive"),
            Self::HostDiskMissing => f.write_str("host disk_bytes is missing or null"),
            Self::HostDiskTotalMissing => f.write_str("host disk_total_bytes is missing or null"),
            Self::RunningLaunchTimingMissing => {
                f.write_str("running box is missing last launch timing")
            }
        }
    }
}

impl std::error::Error for CheckError {}

pub fn parse_and_validate(body: &[u8]) -> Result<(), CheckError> {
    if body.is_empty() {
        return Err(CheckError::EmptyBody);
    }
    let document: Document = serde_json::from_slice(body).map_err(|_| CheckError::InvalidJson)?;
    validate_document(&document)
}

pub(crate) fn validate_document(document: &Document) -> Result<(), CheckError> {
    let metrics = document.host_metrics();
    if metrics.cpu_percent.is_none() {
        return Err(CheckError::HostCpuMissing);
    }
    let memory = metrics
        .memory_bytes
        .filter(|value| *value > 0)
        .ok_or(CheckError::HostMemoryMissing)?;
    if memory == 0 {
        return Err(CheckError::HostMemoryNotPositive);
    }
    if metrics.disk_bytes.filter(|value| *value > 0).is_none() {
        return Err(CheckError::HostDiskMissing);
    }
    if metrics
        .disk_total_bytes
        .filter(|value| *value > 0)
        .is_none()
    {
        return Err(CheckError::HostDiskTotalMissing);
    }
    if document.running_without_launch_timing() {
        return Err(CheckError::RunningLaunchTimingMissing);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::{self, BoxFacts, Inventory};
    use crate::metrics::Snapshot;
    use serde_json::json;

    #[test]
    fn server_built_document_survives_gateway_reserialize() {
        let observed = dashboard::utc_timestamp(1_729_512_001);
        let inventory = Inventory {
            boxes: vec![
                BoxFacts {
                    id: "box-running".into(),
                    name: "smoke".into(),
                    state: "running".into(),
                    memory_mib: 4096,
                    vcpus: 1,
                    disk_bytes: Some(10_737_418_240),
                    created_at: 1_729_507_200,
                    last_operation: None,
                    last_error: None,
                    restored_from: None,
                    metrics: Snapshot {
                        cpu_percent: None,
                        memory_bytes: Some(512_000_000),
                        disk_bytes: None,
                        disk_total_bytes: None,
                    },
                    timing: Some(kiln_runtime::launch_timing::LaunchTiming {
                        method: "create".into(),
                        cache: "uncontrolled".into(),
                        disk_copy: "Copy".into(),
                        storage_ms: Some(1.),
                        vmm_ms: Some(1.),
                        restore_ms: None,
                        agent_ms: None,
                        init_ms: None,
                        first_command_ms: None,
                        total_ms: Some(10.),
                    }),
                },
                BoxFacts {
                    id: "box-stopped".into(),
                    name: "idle".into(),
                    state: "stopped".into(),
                    memory_mib: 4096,
                    vcpus: 1,
                    disk_bytes: None,
                    created_at: 1_729_503_600,
                    last_operation: None,
                    last_error: None,
                    restored_from: None,
                    metrics: Snapshot::default(),
                    timing: None,
                },
            ],
            ..Default::default()
        };
        let document = dashboard::document(
            "local",
            &observed,
            "online",
            Snapshot {
                cpu_percent: Some(0.42),
                memory_bytes: Some(5000),
                disk_bytes: Some(4000),
                disk_total_bytes: Some(8000),
            },
            &inventory,
        );
        let mut value = serde_json::to_value(&document).unwrap();
        dashboard::stamp_value(&mut value, "local", &observed);
        let bytes = serde_json::to_vec(&value).unwrap();
        parse_and_validate(&bytes).expect("gateway-shaped server document");
    }

    #[test]
    fn running_box_null_cpu_does_not_fail_host_gate() {
        let body = json!({
            "schema_version": 1,
            "version": "0.4.17",
            "observed_at": "2026-10-21T12:00:01Z",
            "workspace": "local",
            "host": {
                "name": "local",
                "state": "online",
                "metrics": {
                    "cpu_percent": 1.0,
                    "memory_bytes": 5000,
                    "disk_bytes": 4000,
                    "disk_total_bytes": 8000
                }
            },
            "boxes": [{
                "id": "b1",
                "name": "smoke",
                "state": "running",
                "image": "",
                "resources": null,
                "metrics": { "cpu_percent": null, "memory_bytes": 100, "disk_bytes": null },
                "created_at": "2026-10-21T12:00:00Z",
                "last_operation": null,
                "last_error": null,
                "restored_from": null,
                "timing": {
                    "method": "journal",
                    "cache": "unmeasured",
                    "disk_copy": "Copy",
                    "total_ms": 1000
                }
            }],
            "templates": [],
            "checkpoints": [],
            "operations": []
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        parse_and_validate(&bytes).unwrap();
    }
}
