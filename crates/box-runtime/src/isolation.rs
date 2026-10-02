use crate::{Error, Result, host, process::FileIdentity, storage};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub firecracker: PathBuf,
    pub jailer: PathBuf,
    pub cgroup_parent: String,
    pub uid_base: u32,
    pub gid_base: u32,
    #[serde(default, skip_serializing_if = "DiskBackend::is_copy")]
    pub disk_backend: DiskBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::network::Config>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskBackend {
    #[default]
    Copy,
    Snapshot,
}

impl DiskBackend {
    fn is_copy(&self) -> bool {
        *self == Self::Copy
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JailIdentity {
    pub uid: u32,
    pub gid: u32,
}

// Privileged configuration and state cannot reside below an operator-writable
// directory. Check every component, not just the leaf, and reject aliases.
pub fn trusted_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || fs::canonicalize(path)? != path {
        return Err(Error::Invalid(
            "isolated paths must be absolute and canonical".into(),
        ));
    }
    for component in path.ancestors() {
        let metadata = fs::symlink_metadata(component)?;
        if metadata.file_type().is_symlink() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0
        {
            return Err(Error::Invalid(format!(
                "{} must be root-owned and not group/other writable",
                component.display()
            )));
        }
    }
    Ok(())
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        require_root()?;
        trusted_path(path)?;
        let config: Self = storage::read_json(path)?;
        config.validate_values()?;
        Ok(config)
    }

    fn validate_values(&self) -> Result<()> {
        if self.uid_base == 0
            || self.gid_base == 0
            || self.uid_base > u32::MAX - 8
            || self.gid_base > u32::MAX - 8
            || self.cgroup_parent.is_empty()
            || self.cgroup_parent.starts_with('.')
            || !self
                .cgroup_parent
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || !self.firecracker.is_absolute()
            || !self.jailer.is_absolute()
            || self.firecracker.file_name().and_then(|s| s.to_str()) != Some("firecracker")
        {
            return Err(Error::Invalid(
                "invalid isolated UID/GID range, binary path, or cgroup parent".into(),
            ));
        }
        if let Some(network) = &self.network {
            network.validate()?;
        }
        Ok(())
    }

    pub fn preflight(&self) -> Result<()> {
        self.validate_values()?;
        require_root()?;
        for (name, path) in [("firecracker", &self.firecracker), ("jailer", &self.jailer)] {
            trusted_path(path)?;
            let metadata = fs::metadata(path)?;
            if !metadata.is_file()
                || metadata.mode() & 0o111 == 0
                || host::executable(name)? != *path
            {
                return Err(Error::Invalid(format!(
                    "PATH must select the configured, executable {name}"
                )));
            }
        }
        if let Some(network) = &self.network {
            trusted_path(&network.helper)?;
            let metadata = fs::metadata(&network.helper)?;
            if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
                return Err(Error::Invalid(
                    "network helper must be a trusted executable".into(),
                ));
            }
        }
        let parent = self.parent();
        trusted_path(&parent)?;
        for path in [
            PathBuf::from("/sys/fs/cgroup/cgroup.subtree_control"),
            parent.join("cgroup.subtree_control"),
        ] {
            let controllers = fs::read_to_string(&path)?;
            if !["cpu", "memory", "pids"]
                .iter()
                .all(|c| controllers.split_whitespace().any(|v| v == *c))
            {
                return Err(Error::Invalid(format!(
                    "{} needs pre-enabled cpu, memory, pids controllers; no automatic host setup",
                    path.display()
                )));
            }
        }
        if !fs::read_to_string(parent.join("cgroup.procs"))?
            .trim()
            .is_empty()
        {
            return Err(Error::Invalid(
                "isolated cgroup parent must contain no processes".into(),
            ));
        }
        let report = host::check();
        if !report.development_ready {
            return Err(Error::Invalid(report.problems.join("; ")));
        }
        if self.disk_backend == DiskBackend::Snapshot {
            storage::overlay::preflight()?;
        }
        Ok(())
    }

    fn parent(&self) -> PathBuf {
        Path::new("/sys/fs/cgroup").join(&self.cgroup_parent)
    }

    pub fn cgroup(&self, run: &str) -> Result<PathBuf> {
        if !storage::valid_id(run) {
            return Err(Error::Invalid("invalid jail generation".into()));
        }
        Ok(self.parent().join(run))
    }

    pub fn allocate(&self, used: &[JailIdentity]) -> Result<JailIdentity> {
        self.validate_values()?;
        (0..8)
            .map(|slot| JailIdentity {
                uid: self.uid_base + slot,
                gid: self.gid_base + slot,
            })
            .find(|next| {
                !used
                    .iter()
                    .any(|old| old.uid == next.uid || old.gid == next.gid)
            })
            .ok_or_else(|| Error::Invalid("isolated identity pool exhausted".into()))
    }

    pub fn validate_identity(&self, identity: &JailIdentity) -> Result<()> {
        self.validate_values()?;
        let slot = identity
            .uid
            .checked_sub(self.uid_base)
            .filter(|slot| *slot < 8);
        if slot.is_none_or(|slot| identity.gid != self.gid_base + slot) {
            return Err(Error::Invalid(
                "box identity is outside configured isolation range".into(),
            ));
        }
        Ok(())
    }

    pub fn args(
        &self,
        base: &Path,
        run: &str,
        identity: &JailIdentity,
        memory: u32,
        vcpus: u8,
    ) -> Result<Vec<String>> {
        self.validate_identity(identity)?;
        self.cgroup(run)?;
        if !base.is_absolute() || !(128..=4096).contains(&memory) || !(1..=4).contains(&vcpus) {
            return Err(Error::Invalid(
                "invalid jail path or resource limits".into(),
            ));
        }
        Ok(vec![
            "--id".into(),
            run.into(),
            "--exec-file".into(),
            self.firecracker.display().to_string(),
            "--uid".into(),
            identity.uid.to_string(),
            "--gid".into(),
            identity.gid.to_string(),
            "--chroot-base-dir".into(),
            base.display().to_string(),
            "--cgroup-version".into(),
            "2".into(),
            "--parent-cgroup".into(),
            self.cgroup_parent.clone(),
            "--cgroup".into(),
            format!("cpu.max={} 100000", u32::from(vcpus) * 100000),
            "--cgroup".into(),
            format!("memory.max={}", memory_limit(memory)),
            "--cgroup".into(),
            "memory.swap.max=0".into(),
            "--cgroup".into(),
            "pids.max=64".into(),
            "--resource-limit".into(),
            "no-file=256".into(),
            "--".into(),
            "--api-sock".into(),
            "api.sock".into(),
        ])
    }

    pub fn verify(
        &self,
        pid: u32,
        run: &str,
        root: &Path,
        identity: &JailIdentity,
        memory: u32,
        vcpus: u8,
    ) -> Result<()> {
        self.validate_identity(identity)?;
        let proc = PathBuf::from(format!("/proc/{pid}"));
        if FileIdentity::read(&proc.join("root"))? != FileIdentity::read(root)?
            || FileIdentity::read(&proc.join("exe"))?
                != FileIdentity::read(&root.join("firecracker"))?
            || fs::read_to_string(proc.join("cgroup"))?.trim()
                != format!("0::/{}/{run}", self.cgroup_parent)
        {
            return Err(Error::Invalid(
                "VMM jail or cgroup membership mismatch".into(),
            ));
        }
        for task in fs::read_dir(proc.join("task"))? {
            verify_status(&fs::read_to_string(task?.path().join("status"))?, identity)?;
        }
        let group = self.cgroup(run)?;
        for (file, expected) in [
            ("memory.max", memory_limit(memory).to_string()),
            ("cpu.max", format!("{} 100000", u32::from(vcpus) * 100000)),
            ("pids.max", "64".into()),
            ("memory.swap.max", "0".into()),
        ] {
            if fs::read_to_string(group.join(file))?.trim() != expected {
                return Err(Error::Invalid(format!(
                    "VMM resource limit mismatch: {file}"
                )));
            }
        }
        Ok(())
    }

    pub fn cleanup_cgroup(&self, run: &str) -> Result<()> {
        let path = self.cgroup(run)?;
        if !path.exists() {
            return Ok(());
        }
        if !fs::read_to_string(path.join("cgroup.procs"))?
            .trim()
            .is_empty()
        {
            return Err(Error::Invalid(
                "refusing to remove populated VM cgroup".into(),
            ));
        }
        fs::remove_dir(path)?;
        Ok(())
    }
}

fn memory_limit(memory: u32) -> u64 {
    // Guest RAM plus snapshot writeback/page-cache and fixed VMM overhead.
    (u64::from(memory) * 2 + 128) * 1024 * 1024
}

pub fn require_root() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(Error::Invalid("isolated profile requires an explicitly privileged operator; refusing unjailed fallback".into()));
    }
    Ok(())
}

pub fn stage_readonly(
    source: &Path,
    destination: &Path,
    expected: &str,
    seal: Option<&storage::verity::Seal>,
) -> Result<()> {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    if let Some(seal) = seal {
        storage::verity::verify(source, expected, Some(seal))?;
        let metadata = fs::symlink_metadata(source)?;
        if metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o444 {
            match fs::hard_link(source, destination) {
                Ok(()) => {
                    let result = (|| -> Result<()> {
                        // Authenticate the actual linked inode, not just the
                        // source pathname inspected before link creation.
                        storage::verity::verify(destination, expected, Some(seal))?;
                        let metadata = fs::symlink_metadata(destination)?;
                        if !metadata.is_file()
                            || metadata.uid() != 0
                            || metadata.mode() & 0o7777 != 0o444
                        {
                            return Err(Error::Invalid(
                                "sealed jail input ownership/mode changed".into(),
                            ));
                        }
                        fs::File::open(destination.parent().unwrap_or(Path::new(".")))?
                            .sync_all()?;
                        Ok(())
                    })();
                    if result.is_err() {
                        fs::remove_file(destination)?;
                    }
                    return result;
                }
                Err(error) if error.raw_os_error() == Some(libc::EXDEV) => (),
                Err(error) => return Err(error.into()),
            }
        }
    }
    // Unsealed/legacy-mode/cross-filesystem inputs get a private copy whose
    // complete contents are authenticated before being exposed to the jail.
    storage::copy_disk(
        source,
        destination,
        Instant::now() + Duration::from_secs(120),
    )?;
    let result = (|| -> Result<()> {
        if crate::image::sha256(destination)? != expected {
            return Err(Error::Invalid("staged jail input checksum mismatch".into()));
        }
        fs::set_permissions(destination, fs::Permissions::from_mode(0o444))?;
        fs::File::open(destination)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        fs::remove_file(destination)?;
    }
    result
}

pub fn own_file(path: &Path, identity: &JailIdentity) -> Result<()> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(Error::Invalid(
            "jail artifact must be a regular file without hardlinks".into(),
        ));
    }
    use std::os::fd::AsRawFd;
    // SAFETY: the fd pins a regular, non-symlink file; IDs were validated.
    if unsafe { libc::fchown(file.as_raw_fd(), identity.uid, identity.gid) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn verify_status(status: &str, identity: &JailIdentity) -> Result<()> {
    let fields: std::collections::BTreeMap<_, _> = status
        .lines()
        .filter_map(|line| line.split_once(':'))
        .collect();
    for (key, expected) in [("Uid", identity.uid), ("Gid", identity.gid)] {
        let ids: Vec<_> = fields
            .get(key)
            .into_iter()
            .flat_map(|v| v.split_whitespace())
            .collect();
        if ids.len() != 4 || ids.iter().any(|v| *v != expected.to_string()) {
            return Err(Error::Invalid(format!("VMM {key} mismatch")));
        }
    }
    for (key, expected) in [
        ("Groups", ""),
        ("CapEff", "0000000000000000"),
        ("NoNewPrivs", "1"),
        ("Seccomp", "2"),
    ] {
        if fields.get(key).map(|v| v.trim()) != Some(expected) {
            return Err(Error::Invalid(format!("VMM {key} isolation check failed")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            firecracker: "/opt/boxd/firecracker".into(),
            jailer: "/opt/boxd/jailer".into(),
            cgroup_parent: "boxd".into(),
            uid_base: 70000,
            gid_base: 71000,
            disk_backend: DiskBackend::Copy,
            network: None,
        }
    }

    #[test]
    fn disk_backend_is_explicit_and_legacy_policy_keeps_copying() {
        let mut value = serde_json::to_value(config()).unwrap();
        assert!(value.get("disk_backend").is_none());
        let legacy: Config = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(legacy.disk_backend, DiskBackend::Copy);
        value["disk_backend"] = "snapshot".into();
        assert_eq!(
            serde_json::from_value::<Config>(value.clone())
                .unwrap()
                .disk_backend,
            DiskBackend::Snapshot
        );
        value["disk_backend"] = "unverified_reflink".into();
        assert!(serde_json::from_value::<Config>(value).is_err());
    }

    #[test]
    fn rejects_root_overflow_and_cgroup_path_injection() {
        let mut config = config();
        assert!(config.validate_values().is_ok());
        for parent in [
            "",
            "..",
            "../system.slice",
            "/boxd",
            "boxd/other",
            "boxd\nother",
        ] {
            config.cgroup_parent = parent.into();
            assert!(config.validate_values().is_err(), "{parent:?}");
        }
        config = self::config();
        config.uid_base = 0;
        assert!(config.validate_values().is_err());
        config.uid_base = u32::MAX - 3;
        assert!(config.validate_values().is_err());
        config = self::config();
        config.gid_base = 0;
        assert!(config.validate_values().is_err());
    }

    #[test]
    fn allocates_distinct_identities_without_reusing_stopped_boxes() {
        let config = config();
        let first = config.allocate(&[]).unwrap();
        let second = config.allocate(std::slice::from_ref(&first)).unwrap();
        assert_eq!((first.uid, first.gid), (70000, 71000));
        assert_eq!((second.uid, second.gid), (70001, 71001));
        let all: Vec<_> = (0..8)
            .map(|slot| JailIdentity {
                uid: 70000 + slot,
                gid: 71000 + slot,
            })
            .collect();
        assert!(config.allocate(&all).is_err());
    }

    #[test]
    fn staged_inputs_are_independent_readonly_and_checksum_verified() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("archive");
        let target = root.path().join("restore.memory");
        fs::write(&source, b"abc").unwrap();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        stage_readonly(&source, &target, hash, None).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o444);
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&target).unwrap().ino()
        );
        fs::write(&source, b"changed").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"abc");
        let bad = root.path().join("corrupt");
        assert!(stage_readonly(&source, &bad, hash, None).is_err());
        assert!(!bad.exists());
    }

    #[test]
    fn refuses_chown_of_hardlinked_jail_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("disk");
        fs::write(&file, b"private").unwrap();
        fs::hard_link(&file, root.path().join("alias")).unwrap();
        let identity = JailIdentity {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        };
        assert!(own_file(&file, &identity).is_err());
    }

    #[test]
    fn claimed_seal_cannot_authorize_unsealed_jail_inputs() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::write(&source, b"abc").unwrap();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let seal = storage::verity::Seal {
            sha256: hash.into(),
            digest: "00".repeat(32),
        };
        assert!(stage_readonly(&source, &target, hash, Some(&seal)).is_err());
        assert!(!target.exists());
    }

    #[test]
    #[ignore = "requires root and BOXD_TEST_VERITY_DIR on an fs-verity-enabled filesystem"]
    fn sealed_jail_links_preserve_integrity_ownership_and_lifetime() {
        use std::os::unix::fs::PermissionsExt;
        require_root().unwrap();
        let root = tempfile::tempdir_in(std::env::var("BOXD_TEST_VERITY_DIR").unwrap()).unwrap();
        let source = root.path().join("source");
        let a = root.path().join("jail-a");
        let b = root.path().join("jail-b");
        fs::write(&source, b"abc").unwrap();
        let (hash, seal) = storage::verity::seal(&source).unwrap();
        let seal = seal.expect("real sealing required");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
        stage_readonly(&source, &a, &hash, Some(&seal)).unwrap();
        stage_readonly(&source, &b, &hash, Some(&seal)).unwrap();
        assert_eq!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&a).unwrap().ino()
        );
        assert_eq!(
            fs::metadata(&a).unwrap().ino(),
            fs::metadata(&b).unwrap().ino()
        );
        assert_eq!(fs::metadata(&a).unwrap().uid(), 0);
        assert!(fs::OpenOptions::new().write(true).open(&a).is_err());
        let mut bad = seal.clone();
        bad.digest = "00".repeat(32);
        let rejected = root.path().join("bad");
        assert!(stage_readonly(&source, &rejected, &hash, Some(&bad)).is_err());
        assert!(!rejected.exists());
        assert!(stage_readonly(&source, &a, &hash, Some(&seal)).is_err());
        assert_eq!(fs::read(&a).unwrap(), b"abc");
        fs::remove_file(&a).unwrap();
        fs::remove_file(&source).unwrap();
        storage::verity::verify(&b, &hash, Some(&seal)).unwrap();
        assert_eq!(fs::read(&b).unwrap(), b"abc");
        // Old sealed files keep 0400 and fall back to private authenticated copies.
        fs::set_permissions(&b, fs::Permissions::from_mode(0o400)).unwrap();
        let legacy = root.path().join("legacy-mode");
        stage_readonly(&b, &legacy, &hash, Some(&seal)).unwrap();
        assert_ne!(
            fs::metadata(&b).unwrap().ino(),
            fs::metadata(&legacy).unwrap().ino()
        );
    }

    #[test]
    fn foreground_jailer_command_has_fixed_paths_and_explicit_limits() {
        let config = config();
        let id = "0123456789abcdef0123456789abcdef";
        let identity = JailIdentity {
            uid: 70003,
            gid: 71003,
        };
        let large = config
            .args(
                Path::new("/var/lib/boxd/generation"),
                id,
                &identity,
                4096,
                1,
            )
            .unwrap();
        assert!(large.contains(&"memory.max=8724152320".to_owned()));
        assert!(large.contains(&"cpu.max=100000 100000".to_owned()));
        for memory in [127, 4097] {
            assert!(
                config
                    .args(
                        Path::new("/var/lib/boxd/generation"),
                        id,
                        &identity,
                        memory,
                        1
                    )
                    .is_err()
            );
        }
        let args = config
            .args(
                Path::new("/var/lib/boxd/generation"),
                id,
                &JailIdentity {
                    uid: 70003,
                    gid: 71003,
                },
                384,
                2,
            )
            .unwrap();
        assert_eq!(
            args,
            vec![
                "--id",
                id,
                "--exec-file",
                "/opt/boxd/firecracker",
                "--uid",
                "70003",
                "--gid",
                "71003",
                "--chroot-base-dir",
                "/var/lib/boxd/generation",
                "--cgroup-version",
                "2",
                "--parent-cgroup",
                "boxd",
                "--cgroup",
                "cpu.max=200000 100000",
                "--cgroup",
                "memory.max=939524096",
                "--cgroup",
                "memory.swap.max=0",
                "--cgroup",
                "pids.max=64",
                "--resource-limit",
                "no-file=256",
                "--",
                "--api-sock",
                "api.sock"
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
        assert!(
            config
                .args(
                    Path::new("/var/lib/boxd"),
                    "../bad",
                    &JailIdentity { uid: 1, gid: 2 },
                    256,
                    1
                )
                .is_err()
        );
    }

    #[test]
    fn rejects_partial_process_isolation_evidence() {
        let identity = JailIdentity {
            uid: 70000,
            gid: 71000,
        };
        let good = "Uid:\t70000\t70000\t70000\t70000\nGid:\t71000\t71000\t71000\t71000\nGroups:\nCapEff:\t0000000000000000\nNoNewPrivs:\t1\nSeccomp:\t2\n";
        assert!(verify_status(good, &identity).is_ok());
        for bad in [
            good.replace("Seccomp:\t2", "Seccomp:\t0"),
            good.replace("NoNewPrivs:\t1", "NoNewPrivs:\t0"),
            good.replace("Groups:\n", "Groups:\t0\n"),
            good.replacen("70000", "0", 1),
            good.replace("CapEff:\t0000000000000000", "CapEff:\t0000000000000001"),
        ] {
            assert!(verify_status(&bad, &identity).is_err());
        }
        assert!(verify_status("", &identity).is_err());
    }
}
