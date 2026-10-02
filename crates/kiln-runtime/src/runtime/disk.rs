//! Disk ownership across jail generations. A layer's manifest pins its base
//! independently of the current memory snapshot and survives interrupted restore.
use super::*;
use storage::overlay;

impl Runtime {
    pub(super) fn snapshot_disks(&self) -> bool {
        self.isolation
            .as_ref()
            .is_some_and(|c| c.disk_backend == isolation::DiskBackend::Snapshot)
    }

    fn layers(&self, record: &BoxRecord) -> Result<PathBuf> {
        Ok(self.directory(&record.id)?.join("layers"))
    }

    fn layer_directory(&self, record: &BoxRecord) -> Result<PathBuf> {
        let id = record
            .disk_layer
            .as_deref()
            .filter(|id| storage::valid_id(id))
            .ok_or_else(|| Error::Invalid("disk layer missing or invalid".into()))?;
        Ok(self.layers(record)?.join(id))
    }

    pub(super) fn prepare_snapshot_disk(
        &self,
        record: &mut BoxRecord,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let base = self
            .root
            .join("snapshots")
            .join(&snapshot.id)
            .join("disk.ext4");
        let hash = snapshot
            .hashes
            .get("disk.ext4")
            .ok_or_else(|| Error::Invalid("disk hash missing".into()))?;
        if self.snapshot_disks() {
            let seal = snapshot.seals.get("disk.ext4").ok_or_else(|| {
                Error::Invalid("snapshot disk backend requires an fs-verity sealed base".into())
            })?;
            let layers = self.layers(record)?;
            storage::private_dir(&layers)?;
            let id = id()?;
            overlay::create(&layers.join(&id), &snapshot.id, &base, hash, seal)?;
            record.disk_layer = Some(id);
            record.disk_copy = "Snapshot".into();
        } else {
            let method = storage::copy_snapshot_disk(
                &base,
                &self.run(record)?.join("disk.ext4"),
                hash,
                snapshot.seals.get("disk.ext4"),
                Instant::now() + Duration::from_secs(120),
            )?;
            record.disk_layer = None;
            record.disk_copy = format!("{method:?}");
        }
        Ok(())
    }

    pub(super) fn ensure_disk_layer(&self, record: &BoxRecord) -> Result<()> {
        if record.disk_layer.is_none() {
            return Ok(());
        }
        let directory = self.layer_directory(record)?;
        let layer = overlay::read(&directory)?;
        // The caller's boot/disk phase includes this validation; do not nest
        // snapshot-verification phase timers and double-count wall time.
        let snapshot = self.load_snapshot_metadata(&layer.source)?;
        let hash = snapshot
            .hashes
            .get("disk.ext4")
            .ok_or_else(|| Error::Invalid("disk hash missing".into()))?;
        let seal = snapshot
            .seals
            .get("disk.ext4")
            .ok_or_else(|| Error::Invalid("disk seal missing".into()))?;
        overlay::ensure(
            &directory,
            &self
                .root
                .join("snapshots")
                .join(&layer.source)
                .join("disk.ext4"),
            hash,
            seal,
        )?;
        Ok(())
    }

    pub(super) fn expose_disk_layer(&self, record: &BoxRecord) -> Result<()> {
        let identity = record
            .jail
            .as_ref()
            .ok_or_else(|| Error::Invalid("snapshot disks require isolation".into()))?;
        overlay::expose(
            &self.layer_directory(record)?,
            &self.run(record)?.join("disk.ext4"),
            identity.uid,
            identity.gid,
        )
    }

    pub(super) fn check_disk_layer(&self, record: &BoxRecord) -> Result<()> {
        if record.disk_layer.is_some() {
            overlay::check(&self.layer_directory(record)?)?;
        }
        Ok(())
    }

    pub(super) fn capture_disk(&self, record: &BoxRecord, destination: &Path) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(120);
        if record.disk_layer.is_some() {
            self.ensure_disk_layer(record)?;
            overlay::flatten(&self.layer_directory(record)?, destination, deadline)
        } else {
            let disk = self.run(record)?.join("disk.ext4");
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&disk)?
                .sync_all()?;
            storage::copy_disk(&disk, destination, deadline)?;
            Ok(())
        }
    }

    pub(super) fn disk_references(&self, record: &BoxRecord, snapshot: &str) -> Result<bool> {
        let layers = self.layers(record)?;
        if !layers.try_exists()? {
            return Ok(false);
        }
        for entry in fs::read_dir(layers)? {
            let path = entry?.path();
            if path.join("layer.json").try_exists()? && overlay::read(&path)?.source == snapshot {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn cleanup_disk_layers(&self, record: &BoxRecord, keep: Option<&str>) -> Result<()> {
        let layers = self.layers(record)?;
        if !layers.try_exists()? {
            return Ok(());
        }
        storage::private_dir(&layers)?;
        for entry in fs::read_dir(&layers)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .filter(|id| storage::valid_id(id))
                .ok_or_else(|| Error::Invalid("invalid layer directory".into()))?;
            if Some(name) == keep {
                continue;
            }
            storage::private_dir(&path)?;
            if path.join("layer.json").try_exists()? {
                overlay::detach(&path)?;
            }
            // No mapping can be allocated before layer.json is published.
            fs::remove_dir_all(path)?;
            File::open(&layers)?.sync_all()?;
        }
        Ok(())
    }
}
