use crate::{Error, Result};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub development_ready: bool,
    pub isolated_ready: bool,
    pub problems: Vec<String>,
    pub architecture: String,
    pub firecracker_version: String,
    pub jailer_version: String,
}

pub fn evaluate(arch: &str, kvm: bool, cgroup2: bool, firecracker: &str, jailer: &str) -> Report {
    let mut problems = Vec::new();
    if arch != "x86_64" {
        problems.push("only Linux x86_64 is supported".into());
    }
    if !kvm {
        problems.push("/dev/kvm is not readable and writable".into());
    }
    if !cgroup2 {
        problems.push("cgroup v2 is required".into());
    }
    if firecracker != "1.17.0" {
        problems
            .push("Firecracker 1.17.0 is required (1.16.0 breaks vsock after pause/resume)".into());
    }
    if jailer != firecracker {
        problems.push("matching jailer version is required".into());
    }
    Report {
        schema_version: 1,
        development_ready: problems.is_empty(),
        isolated_ready: false,
        problems,
        architecture: arch.into(),
        firecracker_version: firecracker.into(),
        jailer_version: jailer.into(),
    }
}

pub fn executable(name: &str) -> Result<PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(name);
        if path.is_file() {
            return Ok(fs::canonicalize(path)?);
        }
    }
    Err(Error::Invalid(format!("{name} not found in PATH")))
}

fn version(name: &str) -> String {
    executable(name)
        .ok()
        .and_then(|p| Command::new(p).arg("--version").output().ok())
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::to_owned)
        })
        .and_then(|line| {
            line.split_whitespace()
                .nth(1)
                .map(|s| s.trim_start_matches('v').to_owned())
        })
        .unwrap_or_else(|| "unavailable".into())
}

pub fn check() -> Report {
    evaluate(
        std::env::consts::ARCH,
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok(),
        Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
        &version("firecracker"),
        &version("jailer"),
    )
}

pub fn fingerprint() -> Result<String> {
    let cpu = fs::read_to_string("/proc/cpuinfo")?;
    let model: Vec<_> = cpu
        .lines()
        .filter(|line| line.starts_with("model name") || line.starts_with("flags"))
        .take(2)
        .collect();
    Ok(format!(
        "{}|{}|{}|{}",
        std::env::consts::ARCH,
        fs::read_to_string("/proc/sys/kernel/osrelease")?.trim(),
        model.join("|"),
        crate::image::sha256(&executable("firecracker")?)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_distinguishes_development_from_isolation() {
        let report = evaluate("x86_64", true, true, "1.17.0", "1.17.0");
        assert!(report.development_ready);
        assert!(
            !report.isolated_ready,
            "device access is not jail validation"
        );
        assert!(report.problems.is_empty());
    }

    #[test]
    fn preflight_rejects_each_unsupported_condition() {
        for (arch, kvm, cgroup, fc, jailer) in [
            ("aarch64", true, true, "1.17.0", "1.17.0"),
            ("x86_64", false, true, "1.17.0", "1.17.0"),
            ("x86_64", true, false, "1.17.0", "1.17.0"),
            ("x86_64", true, true, "1.16.0", "1.16.0"),
            ("x86_64", true, true, "1.17.0", "1.16.0"),
        ] {
            let report = evaluate(arch, kvm, cgroup, fc, jailer);
            assert!(!report.development_ready);
            assert!(!report.problems.is_empty());
        }
    }
}
