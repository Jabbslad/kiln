use super::*;
use std::os::unix::fs::PermissionsExt;

impl Runtime {
    pub async fn checkpoint(&self, box_id: &str) -> Result<Snapshot> {
        self.capture(box_id, false).await
    }

    async fn capture(&self, box_id: &str, template: bool) -> Result<Snapshot> {
        let _allocation = storage::lock(&self.root)?;
        let _lock = storage::lock(&self.directory(box_id)?)?;
        if fs::read_dir(self.root.join("snapshots"))?.count() >= 16 {
            return Err(Error::Invalid("local snapshot quota exceeded (16)".into()));
        }
        let mut record = self.read(box_id)?;
        self.observed(&mut record).await?;
        if record.state != "running" && record.state != "paused" {
            return Err(Error::Invalid(
                "checkpoint requires a running or paused box".into(),
            ));
        }
        if template
            && !matches!(
                self.guest(
                    &record,
                    &Request::Hello {
                        version: PROTOCOL_VERSION
                    },
                    Duration::from_secs(2)
                )
                .await?,
                Response::Hello {
                    initialized: false,
                    ..
                }
            )
        {
            return Err(Error::Invalid(
                "only an uninitialized builder may become a template".into(),
            ));
        }
        let running = record.state == "running";
        let snapshot_id = id()?;
        let directory = self.root.join("snapshots").join(&snapshot_id);
        storage::private_dir(&directory)?;
        let (_fd, client) = self.connection(&record)?;
        record.operation = Some(
            if running {
                "capture-running"
            } else {
                "capture-paused"
            }
            .into(),
        );
        self.save(&record)?;
        let run = self.run(&record)?;
        // Never overwrite state.snap/memory.snap: they may still back a restored VM.
        let state_output = format!("capture-{snapshot_id}.state");
        let memory_output = format!("capture-{snapshot_id}.memory");
        let result: Result<Snapshot> = async {
            client.request("PATCH", "/vm", json!({"state":"Paused"})).await?;
            record.state = "paused".into(); self.save(&record)?;
            failpoint("after-pause");
            let (state_path, memory_path) = if self.is_isolated() {
                (PathBuf::from(&state_output), PathBuf::from(&memory_output))
            } else {
                (directory.join("state.snap"), directory.join("memory.snap"))
            };
            client.request("PUT", "/snapshot/create", json!({"snapshot_type":"Full", "snapshot_path":state_path, "mem_file_path":memory_path})).await?;
            if self.is_isolated() {
                for (source, destination) in [(&state_output, "state.snap"), (&memory_output, "memory.snap")] {
                    storage::copy_disk(&run.join(source), &directory.join(destination), Instant::now() + Duration::from_secs(120))?;
                }
            }
            self.capture_disk(&record, &directory.join("disk.ext4"))?;
            let mut hashes = std::collections::BTreeMap::new();
            let mut seals = std::collections::BTreeMap::new();
            for name in ["disk.ext4", "state.snap", "memory.snap"] {
                let path = directory.join(name);
                let (hash, seal) = storage::verity::seal(&path)?;
                hashes.insert(name.into(), hash);
                if let Some(seal) = seal { seals.insert(name.into(), seal); }
                // Only immutable root-owned inputs may be shared with a jailed
                // VMM. The private snapshot directory still prevents traversal.
                let mode = if self.is_isolated() && name != "disk.ext4" && seals.contains_key(name) { 0o444 } else { 0o400 };
                fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
                File::open(&path)?.sync_all()?;
            }
            let snapshot = Snapshot {schema_version:1,id:snapshot_id.clone(),source_box:box_id.into(),template,image:record.image.clone(),host:record.host.clone(),memory_mib:record.memory_mib,vcpus:record.vcpus,hashes,seals,isolated:self.is_isolated(),network:record.network.clone()};
            storage::atomic_json(&directory.join("snapshot.json"), &snapshot)?;
            File::open(self.root.join("snapshots"))?.sync_all()?;
            failpoint("after-snapshot-publication");
            Ok(snapshot)
        }.await;
        if self.is_isolated() {
            for name in [&state_output, &memory_output] {
                match fs::remove_file(run.join(name)) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        // Even a failed pause/snapshot request can have changed the VMM. Query
        // it before choosing the recovery action; never retry snapshot creation.
        self.observed(&mut record).await?;
        if running && record.state == "paused" {
            if let Err(error) = client
                .request("PATCH", "/vm", json!({"state":"Resumed"}))
                .await
            {
                record.last_error = Some(format!("source resume failed: {error}"));
                self.save(&record)?;
                return Err(error);
            }
            record.state = "running".into();
        }
        record.operation = None;
        record.last_error = result.as_ref().err().map(ToString::to_string);
        self.save(&record)?;
        if result.is_err() {
            fs::remove_dir_all(&directory)?;
        }
        result
    }

    pub fn snapshots(&self) -> Result<Vec<Snapshot>> {
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(self.root.join("snapshots"))? {
            let entry = entry?;
            if entry.path().join("snapshot.json").is_file() {
                snapshots.push(storage::read_json(&entry.path().join("snapshot.json"))?);
            }
        }
        Ok(snapshots)
    }

    pub fn delete_snapshot(&self, snapshot_id: &str) -> Result<()> {
        if !storage::valid_id(snapshot_id) {
            return Err(Error::Invalid("invalid snapshot ID".into()));
        }
        let _allocation = storage::lock(&self.root)?;
        for record in self.records()? {
            if record.source.as_deref() == Some(snapshot_id)
                || self.disk_references(&record, snapshot_id)?
            {
                return Err(Error::Invalid(
                    "snapshot is referenced by a box; delete that box first".into(),
                ));
            }
        }
        let directory = self.root.join("snapshots").join(snapshot_id);
        if !directory.exists() {
            return Err(Error::Invalid("snapshot not found".into()));
        }
        storage::private_dir(&directory)?;
        fs::remove_dir_all(directory)?;
        File::open(self.root.join("snapshots"))?.sync_all()?;
        Ok(())
    }

    pub(super) fn snapshot_metadata(&self, snapshot_id: &str) -> Result<Snapshot> {
        let _phase = Phase::start("snapshot_verification");
        self.load_snapshot_metadata(snapshot_id)
    }

    pub(super) fn load_snapshot_metadata(&self, snapshot_id: &str) -> Result<Snapshot> {
        if !storage::valid_id(snapshot_id) {
            return Err(Error::Invalid("invalid snapshot ID".into()));
        }
        if let Some(config) = &self.isolation {
            config.preflight()?;
        }
        let report = host::check();
        if !report.development_ready {
            return Err(Error::Invalid(report.problems.join("; ")));
        }
        let directory = self.root.join("snapshots").join(snapshot_id);
        storage::private_dir(&directory)?;
        let snapshot: Snapshot = storage::read_json(&directory.join("snapshot.json"))?;
        if snapshot.schema_version != 1
            || snapshot.id != snapshot_id
            || snapshot.host != host::fingerprint()?
            || snapshot.isolated != self.is_isolated()
            || snapshot.network.is_some()
                != self
                    .isolation
                    .as_ref()
                    .and_then(|c| c.network.as_ref())
                    .is_some()
        {
            return Err(Error::Invalid("snapshot compatibility mismatch".into()));
        }
        if let (Some(config), Some(topology)) = (
            self.isolation.as_ref().and_then(|c| c.network.as_ref()),
            &snapshot.network,
        ) {
            topology.validate(&config.namespace_scope)?;
        }
        Ok(snapshot)
    }

    fn verify_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let _phase = Phase::start("snapshot_verification");
        let directory = self.root.join("snapshots").join(&snapshot.id);
        for name in ["disk.ext4", "state.snap", "memory.snap"] {
            let hash = snapshot
                .hashes
                .get(name)
                .ok_or_else(|| Error::Invalid(format!("snapshot hash missing: {name}")))?;
            storage::verity::verify(&directory.join(name), hash, snapshot.seals.get(name))
                .map_err(|e| Error::Invalid(format!("snapshot integrity failure: {name}: {e}")))?;
        }
        Ok(())
    }

    pub async fn restore(&self, box_id: &str, snapshot_id: &str) -> Result<BoxRecord> {
        let _allocation = storage::lock(&self.root)?;
        let _lock = storage::lock(&self.directory(box_id)?)?;
        let snapshot = self.snapshot_metadata(snapshot_id)?;
        self.verify_snapshot(&snapshot)?;
        if snapshot.template || snapshot.source_box != box_id {
            return Err(Error::Invalid(
                "checkpoint belongs to another box or is a template".into(),
            ));
        }
        let mut record = self.read(box_id)?;
        self.observed(&mut record).await?;
        let previous = record.clone();
        let next_run = id()?;
        let mut next = record.clone();
        next.run = next_run.clone();
        self.prepare_snapshot_disk(&mut next, &snapshot)?;
        self.ensure_disk_layer(&next)?;
        storage::atomic_json(
            &self.directory(box_id)?.join("restore.previous.json"),
            &previous,
        )?;
        record.operation = Some("restore".into());
        self.save(&record)?;
        self.terminate(&mut record)?;
        record.run = next_run;
        record.disk_copy = next.disk_copy;
        record.disk_layer = next.disk_layer;
        record.source = Some(snapshot_id.into());
        record.state = "restoring".into();
        self.save(&record)?;
        let result = self.boot(&mut record, Some(&snapshot), false).await;
        self.finish_launch(&mut record, result)?;
        fs::remove_dir_all(self.directory(box_id)?.join(previous.run))?;
        self.cleanup_disk_layers(&record, record.disk_layer.as_deref())?;
        fs::remove_file(self.directory(box_id)?.join("restore.previous.json"))?;
        Ok(record)
    }

    pub async fn build_template(
        &self,
        image: &Path,
        memory_mib: u32,
        vcpus: u8,
    ) -> Result<Snapshot> {
        let builder = self
            .create(image, "template-builder", memory_mib, vcpus, true)
            .await?;
        let result = self.capture(&builder.id, true).await;
        self.delete(&builder.id).await?;
        result
    }

    pub async fn clone_template(&self, snapshot_id: &str, name: &str) -> Result<BoxRecord> {
        self.clone_template_with_id(snapshot_id, name, &id()?).await
    }

    /// Reserve a service-assigned identity. Existing directories (including
    /// interrupted reservations) are never overwritten or silently reused.
    pub async fn clone_template_with_id(
        &self,
        snapshot_id: &str,
        name: &str,
        box_id: &str,
    ) -> Result<BoxRecord> {
        if !storage::valid_id(box_id) {
            return Err(Error::Invalid("invalid box ID".into()));
        }
        if name.is_empty() || name.len() > 63 {
            return Err(Error::Invalid("name must be 1..63 bytes".into()));
        }
        let phase = Phase::start("allocation_wait");
        let allocation = self.allocation().await?;
        drop(phase);
        if fs::symlink_metadata(self.root.join("boxes").join(box_id)).is_ok() {
            return Err(Error::Invalid("box ID already reserved".into()));
        }
        let snapshot = self.snapshot_metadata(snapshot_id)?;
        let phase = Phase::start("reservation");
        if !snapshot.template {
            return Err(Error::Invalid(
                "only prepared templates can be cloned".into(),
            ));
        }
        let records = self.records()?;
        if records.len() >= 8
            || records.iter().map(|r| r.memory_mib).sum::<u32>() + snapshot.memory_mib > 16384
        {
            return Err(Error::Invalid("local box quota exceeded".into()));
        }
        let mut record = BoxRecord {
            schema_version: 1,
            id: box_id.into(),
            name: name.into(),
            state: "creating".into(),
            run: id()?,
            process: None,
            image: snapshot.image.clone(),
            host: snapshot.host.clone(),
            memory_mib: snapshot.memory_mib,
            vcpus: snapshot.vcpus,
            source: Some(snapshot.id.clone()),
            operation: Some("clone".into()),
            last_error: None,
            disk_copy: String::new(),
            disk_layer: None,
            jail: self.allocate_identity(&records)?,
            network: if snapshot.network.is_some() {
                self.allocate_network(&records)?
            } else {
                None
            },
        };
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(self.root.join("boxes").join(&record.id))?;
        File::open(self.root.join("boxes"))?.sync_all()?;
        let _lock = storage::lock(&self.directory(&record.id)?)?;
        self.save(&record)?;
        // The durable source reference prevents deletion of backing files;
        // quota is reserved and the box lock prevents lifecycle interference.
        // Expensive checksums can now run alongside other box allocations.
        drop(allocation);
        drop(phase);
        let result = async {
            self.verify_snapshot(&snapshot)?;
            let phase = Phase::start("disk_copy");
            self.prepare_snapshot_disk(&mut record, &snapshot)?;
            self.save(&record)?;
            failpoint("after-layer-publication");
            drop(phase);
            self.boot(&mut record, Some(&snapshot), true).await
        }
        .await;
        self.finish_launch(&mut record, result)?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn caller_ids_cannot_escape_or_reuse_a_box_directory() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::open(root.path()).unwrap();
        let invalid = runtime
            .clone_template_with_id("missing", "box", "../outside")
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("invalid box ID"), "{invalid}");
        let existing = "b".repeat(32);
        let directory = root.path().join("boxes").join(&existing);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("sentinel"), "keep").unwrap();
        let conflict = runtime
            .clone_template_with_id("missing", "box", &existing)
            .await
            .unwrap_err();
        assert!(
            conflict.to_string().contains("already reserved"),
            "{conflict}"
        );
        assert_eq!(
            fs::read_to_string(directory.join("sentinel")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn snapshot_verification_honors_seals_and_accepts_legacy_manifests() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::open(root.path()).unwrap();
        let id = "123456789abcdef0123456789abcdef0";
        let directory = root.path().join("snapshots").join(id);
        fs::create_dir(&directory).unwrap();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for name in ["disk.ext4", "state.snap", "memory.snap"] {
            fs::write(directory.join(name), b"abc").unwrap();
        }
        let mut value = json!({
            "schema_version": 1, "id": id, "source_box": id, "template": true,
            "image": {"schema_version": 1, "architecture": "x86_64",
                "kernel_path": "kernel", "kernel_sha256": hash,
                "rootfs_path": "disk", "rootfs_sha256": hash, "agent_protocol_version": 1},
            "host": "test", "memory_mib": 256, "vcpus": 1,
            "hashes": {"disk.ext4": hash, "state.snap": hash, "memory.snap": hash}
        });
        let legacy: Snapshot = serde_json::from_value(value.clone()).unwrap();
        runtime.verify_snapshot(&legacy).unwrap();
        value["seals"] = json!({"memory.snap": {"sha256": hash, "digest": "00".repeat(32)}});
        let claimed: Snapshot = serde_json::from_value(value.clone()).unwrap();
        assert!(
            runtime.verify_snapshot(&claimed).is_err(),
            "must not ignore a claimed seal"
        );
        value["seals"]["memory.snap"]["sha256"] = "11".repeat(32).into();
        let mismatched: Snapshot = serde_json::from_value(value).unwrap();
        assert!(
            runtime
                .verify_snapshot(&mismatched)
                .unwrap_err()
                .to_string()
                .contains("seal SHA-256 mismatch")
        );
        fs::write(directory.join("disk.ext4"), b"abd").unwrap();
        assert!(runtime.verify_snapshot(&legacy).is_err());
    }
}
