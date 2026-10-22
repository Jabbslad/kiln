use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct VerifiedTemplate {
    snapshot_id: String,
    hashes: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct GoldenTemplate {
    snapshot_id: String,
    fingerprint: String,
}

pub(super) fn template_fingerprint(
    manifest: &image::Manifest,
    memory_mib: u32,
    vcpus: u8,
    host: &str,
    isolated: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(manifest.kernel_sha256.as_bytes());
    hasher.update(manifest.rootfs_sha256.as_bytes());
    hasher.update(boot_mode_tag(&manifest.boot_mode).as_bytes());
    hasher.update(memory_mib.to_be_bytes());
    hasher.update([vcpus]);
    hasher.update(host.as_bytes());
    hasher.update([u8::from(isolated)]);
    format!("{:x}", hasher.finalize())
}

fn boot_mode_tag(mode: &image::BootMode) -> &'static str {
    match mode {
        image::BootMode::Init => "init",
        image::BootMode::Systemd => "systemd",
        image::BootMode::SystemdWarm => "systemd_warm",
        image::BootMode::SystemdWarmShared => "systemd_warm_shared",
    }
}

impl Runtime {
    fn golden_dir(&self) -> PathBuf {
        self.root.join("snapshots").join("golden")
    }

    fn verified_dir(&self) -> PathBuf {
        self.root.join("snapshots").join("verified")
    }

    fn published_image_manifest(&self) -> Option<PathBuf> {
        let path = self.root.parent()?.join("image/image.json");
        path.is_file().then_some(path)
    }

    pub(super) fn verified_template_snapshot(&self, snapshot_id: &str) -> Result<Snapshot> {
        let snapshot = self.load_snapshot_metadata(snapshot_id)?;
        if !snapshot.template {
            return Err(Error::Invalid(
                "only prepared templates can be cloned".into(),
            ));
        }
        let fingerprint = template_fingerprint(
            &snapshot.image,
            snapshot.memory_mib,
            snapshot.vcpus,
            &snapshot.host,
            snapshot.isolated,
        );
        let cache_path = self.verified_dir().join(format!("{fingerprint}.json"));
        if cache_path.is_file() {
            let cached: VerifiedTemplate = storage::read_json(&cache_path)?;
            if cached.snapshot_id == snapshot_id && cached.hashes == snapshot.hashes {
                return Ok(snapshot);
            }
        }
        self.verify_snapshot(&snapshot)?;
        storage::private_dir(&self.verified_dir())?;
        storage::atomic_json(
            &cache_path,
            &VerifiedTemplate {
                snapshot_id: snapshot_id.into(),
                hashes: snapshot.hashes.clone(),
            },
        )?;
        Ok(snapshot)
    }

    fn read_golden_index(&self, fingerprint: &str) -> Result<Option<GoldenTemplate>> {
        let path = self.golden_dir().join(format!("{fingerprint}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let entry: GoldenTemplate = storage::read_json(&path)?;
        if entry.fingerprint == fingerprint {
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    fn write_golden_index(&self, fingerprint: &str, snapshot_id: &str) -> Result<()> {
        storage::private_dir(&self.golden_dir())?;
        storage::atomic_json(
            &self.golden_dir().join(format!("{fingerprint}.json")),
            &GoldenTemplate {
                snapshot_id: snapshot_id.into(),
                fingerprint: fingerprint.into(),
            },
        )
    }

    fn snapshot_matches_manifest(&self, snapshot: &Snapshot, manifest: &image::Manifest) -> bool {
        snapshot.template
            && snapshot.image.kernel_sha256 == manifest.kernel_sha256
            && snapshot.image.rootfs_sha256 == manifest.rootfs_sha256
            && snapshot.image.boot_mode == manifest.boot_mode
            && snapshot.host == host::fingerprint().unwrap_or_default()
            && snapshot.isolated == self.is_isolated()
    }

    pub async fn ensure_template_for_create(&self, configured_id: &str) -> Result<String> {
        if !storage::valid_id(configured_id) {
            return Err(Error::Invalid("invalid template ID".into()));
        }
        let Some(manifest_path) = self.published_image_manifest() else {
            self.verified_template_snapshot(configured_id)?;
            return Ok(configured_id.into());
        };
        let manifest = image::load(&manifest_path)?;
        let configured = self.load_snapshot_metadata(configured_id)?;
        if !configured.template {
            return Err(Error::Invalid(
                "catalog template is not a prepared golden snapshot".into(),
            ));
        }
        let fingerprint = template_fingerprint(
            &manifest,
            configured.memory_mib,
            configured.vcpus,
            &host::fingerprint()?,
            self.is_isolated(),
        );
        if self.snapshot_matches_manifest(&configured, &manifest) {
            self.verified_template_snapshot(configured_id)?;
            self.write_golden_index(&fingerprint, configured_id)?;
            return Ok(configured_id.into());
        }
        if let Some(index) = self.read_golden_index(&fingerprint)? {
            if self.load_snapshot_metadata(&index.snapshot_id).is_ok() {
                let candidate = self.verified_template_snapshot(&index.snapshot_id)?;
                if self.snapshot_matches_manifest(&candidate, &manifest) {
                    return Ok(index.snapshot_id);
                }
            }
        }
        let snapshot = self
            .build_template(&manifest_path, configured.memory_mib, configured.vcpus)
            .await?;
        self.write_golden_index(&fingerprint, &snapshot.id)?;
        self.verified_template_snapshot(&snapshot.id)?;
        Ok(snapshot.id)
    }

    pub(super) fn register_golden_template(&self, snapshot: &Snapshot) -> Result<()> {
        if !snapshot.template {
            return Ok(());
        }
        let fingerprint = template_fingerprint(
            &snapshot.image,
            snapshot.memory_mib,
            snapshot.vcpus,
            &snapshot.host,
            snapshot.isolated,
        );
        self.write_golden_index(&fingerprint, &snapshot.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_index_round_trip() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::open(root.path()).unwrap();
        let id = "123456789abcdef0123456789abcdef0";
        let fingerprint = "abc123";
        runtime.write_golden_index(fingerprint, id).unwrap();
        let entry = runtime.read_golden_index(fingerprint).unwrap().unwrap();
        assert_eq!(entry.snapshot_id, id);
        assert_eq!(entry.fingerprint, fingerprint);
    }

    #[test]
    fn template_fingerprint_changes_when_rootfs_hash_changes() {
        let mut manifest = image::Manifest {
            schema_version: 1,
            architecture: "x86_64".into(),
            kernel_path: "k".into(),
            kernel_sha256: "aa".repeat(32),
            rootfs_path: "r".into(),
            rootfs_sha256: "bb".repeat(32),
            agent_protocol_version: 1,
            boot_mode: image::BootMode::SystemdWarm,
        };
        let first = template_fingerprint(&manifest, 4096, 1, "host", false);
        manifest.rootfs_sha256 = "cc".repeat(32);
        let second = template_fingerprint(&manifest, 4096, 1, "host", false);
        assert_ne!(first, second);
    }
}
