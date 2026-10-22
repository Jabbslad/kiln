use super::*;
use std::os::unix::fs::PermissionsExt;

const MIN_FREE_FRACTION: u128 = 20;

pub(super) fn saving_would_violate_disk_guard(free: u128, total: u128, required: u128) -> bool {
    free.saturating_sub(required).saturating_mul(100) < total.saturating_mul(MIN_FREE_FRACTION)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopState {
    pub schema_version: u32,
    pub run: String,
    pub host: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    pub hashes: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub seals: std::collections::BTreeMap<String, storage::verity::Seal>,
}

impl Runtime {
    pub(super) fn saved_state_dir(&self, box_id: &str) -> Result<PathBuf> {
        Ok(self.directory(box_id)?.join("saved"))
    }

    pub(super) fn stop_state(&self, box_id: &str) -> Result<Option<StopState>> {
        let directory = self.saved_state_dir(box_id)?;
        let manifest = directory.join("saved.json");
        if !manifest.is_file() {
            return Ok(None);
        }
        storage::private_dir(&directory)?;
        let state: StopState = storage::read_json(&manifest)?;
        if state.schema_version != 1 || !storage::valid_id(&state.run) {
            return Err(Error::Invalid("invalid saved stop state".into()));
        }
        Ok(Some(state))
    }

    pub(super) fn remove_saved_state(&self, box_id: &str) -> Result<()> {
        let directory = self.saved_state_dir(box_id)?;
        if directory.try_exists()? {
            storage::private_dir(&directory)?;
            fs::remove_dir_all(&directory)?;
            File::open(self.directory(box_id)?)?.sync_all()?;
        }
        Ok(())
    }

    pub(super) fn memory_save_would_violate_disk_guard(
        storage_root: &Path,
        required_bytes: u64,
    ) -> Result<bool> {
        let file = File::open(storage_root)?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stats = unsafe { stats.assume_init() };
        if stats.f_frsize == 0 || stats.f_blocks == 0 {
            return Err(Error::Invalid("storage volume has no capacity".into()));
        }
        let unit = stats.f_frsize as u128;
        let total = (stats.f_blocks as u128).saturating_mul(unit);
        let free = (stats.f_bavail as u128).saturating_mul(unit);
        let required = required_bytes as u128;
        Ok(saving_would_violate_disk_guard(free, total, required))
    }

    fn kiln_storage_root(&self) -> Result<PathBuf> {
        self.root
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| Error::Invalid("runtime root has no parent directory".into()))
    }

    pub(super) async fn persist_stop_state(&self, record: &mut BoxRecord) -> Result<()> {
        let required = (record.memory_mib as u64)
            .saturating_mul(1024 * 1024)
            .saturating_add(16 * 1024 * 1024);
        if Self::memory_save_would_violate_disk_guard(&self.kiln_storage_root()?, required)? {
            self.remove_saved_state(&record.id)?;
            return Ok(());
        }
        let directory = self.saved_state_dir(&record.id)?;
        storage::private_dir(&directory)?;
        self.remove_saved_state(&record.id)?;
        storage::private_dir(&directory)?;
        let (_fd, client) = self.connection(record)?;
        let run = self.run(record)?;
        let state_output = "stop-capture.state";
        let memory_output = "stop-capture.memory";
        client
            .request("PATCH", "/vm", json!({"state":"Paused"}))
            .await?;
        let (state_path, memory_path) = if self.is_isolated() {
            (PathBuf::from(state_output), PathBuf::from(memory_output))
        } else {
            (directory.join("state.snap"), directory.join("memory.snap"))
        };
        client
            .request(
                "PUT",
                "/snapshot/create",
                json!({"snapshot_type":"Full", "snapshot_path":state_path, "mem_file_path":memory_path}),
            )
            .await?;
        if self.is_isolated() {
            for (source, destination) in
                [(state_output, "state.snap"), (memory_output, "memory.snap")]
            {
                storage::copy_disk(
                    &run.join(source),
                    &directory.join(destination),
                    Instant::now() + Duration::from_secs(120),
                )?;
            }
        }
        if record.disk_layer.is_some() {
            self.ensure_disk_layer(record)?;
        }
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(run.join("disk.ext4"))?
            .sync_all()?;
        let mut hashes = std::collections::BTreeMap::new();
        let mut seals = std::collections::BTreeMap::new();
        for name in ["state.snap", "memory.snap"] {
            let path = directory.join(name);
            let (hash, seal) = storage::verity::seal(&path)?;
            hashes.insert(name.into(), hash);
            if let Some(seal) = seal {
                seals.insert(name.into(), seal);
            }
            let mode = if self.is_isolated() && seals.contains_key(name) {
                0o444
            } else {
                0o400
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
            File::open(&path)?.sync_all()?;
        }
        let state = StopState {
            schema_version: 1,
            run: record.run.clone(),
            host: record.host.clone(),
            memory_mib: record.memory_mib,
            vcpus: record.vcpus,
            hashes,
            seals,
        };
        storage::atomic_json(&directory.join("saved.json"), &state)?;
        File::open(&directory)?.sync_all()?;
        if self.is_isolated() {
            for name in [state_output, memory_output] {
                match fs::remove_file(run.join(name)) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    pub(super) fn verify_stop_state(&self, state: &StopState, record: &BoxRecord) -> Result<()> {
        if state.run != record.run
            || state.host != record.host
            || state.memory_mib != record.memory_mib
            || state.vcpus != record.vcpus
        {
            return Err(Error::Invalid("saved stop state mismatch".into()));
        }
        let directory = self.saved_state_dir(&record.id)?;
        for name in ["state.snap", "memory.snap"] {
            let hash = state
                .hashes
                .get(name)
                .ok_or_else(|| Error::Invalid(format!("saved state hash missing: {name}")))?;
            storage::verity::verify(&directory.join(name), hash, state.seals.get(name)).map_err(
                |e| Error::Invalid(format!("saved state integrity failure: {name}: {e}")),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn disk_guard_skips_save_when_free_space_would_drop_below_twenty_percent() {
        assert!(!saving_would_violate_disk_guard(500, 1000, 100));
        assert!(saving_would_violate_disk_guard(850, 1000, 801));
        assert!(!saving_would_violate_disk_guard(250, 1000, 0));
    }

    #[test]
    fn delete_removes_saved_memory_artifacts() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let boxes = root.path().join("boxes");
        fs::create_dir_all(&boxes).unwrap();
        fs::set_permissions(&boxes, fs::Permissions::from_mode(0o700)).unwrap();
        let snapshots = root.path().join("snapshots");
        fs::create_dir_all(&snapshots).unwrap();
        fs::set_permissions(&snapshots, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::open(root.path()).unwrap();
        let id = "a".repeat(32);
        let directory = root.path().join("boxes").join(&id);
        storage::private_dir(&directory).unwrap();
        let saved = directory.join("saved");
        storage::private_dir(&saved).unwrap();
        fs::write(saved.join("memory.snap"), b"sparse").unwrap();
        fs::write(saved.join("state.snap"), b"state").unwrap();
        storage::atomic_json(
            &saved.join("saved.json"),
            &StopState {
                schema_version: 1,
                run: "b".repeat(32),
                host: "test".into(),
                memory_mib: 256,
                vcpus: 1,
                hashes: Default::default(),
                seals: Default::default(),
            },
        )
        .unwrap();
        runtime.remove_saved_state(&id).unwrap();
        assert!(!saved.exists());
    }

    #[test]
    fn boxes_stopped_before_upgrade_have_no_saved_state() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let boxes = root.path().join("boxes");
        fs::create_dir_all(&boxes).unwrap();
        fs::set_permissions(&boxes, fs::Permissions::from_mode(0o700)).unwrap();
        let snapshots = root.path().join("snapshots");
        fs::create_dir_all(&snapshots).unwrap();
        fs::set_permissions(&snapshots, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::open(root.path()).unwrap();
        let id = "c".repeat(32);
        let directory = root.path().join("boxes").join(&id);
        storage::private_dir(&directory).unwrap();
        assert!(runtime.stop_state(&id).unwrap().is_none());
    }
}
