use crate::{
    Error, Result,
    firecracker::Client,
    guest, host,
    image::{self, Manifest},
    isolation::{self, Config, JailIdentity},
    process::{self, Identity},
    storage,
};
use box_protocol::{
    ExecRequest, ExecResult, InitializeRequest, PROTOCOL_VERSION, Request, Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
mod benchmark;
mod checkpoint;
use benchmark::Phase;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxRecord {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub state: String,
    pub run: String,
    pub process: Option<Identity>,
    pub image: Manifest,
    pub host: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    pub source: Option<String>,
    pub operation: Option<String>,
    pub last_error: Option<String>,
    pub disk_copy: String,
    #[serde(default)]
    pub jail: Option<JailIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: String,
    pub source_box: String,
    pub template: bool,
    pub image: Manifest,
    pub host: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    pub hashes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub isolated: bool,
}

pub struct Runtime {
    root: PathBuf,
    isolation: Option<Config>,
}

pub fn random_bytes(count: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; count];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn id() -> Result<String> {
    Ok(random_bytes(16)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn failpoint(_name: &str) {
    #[cfg(feature = "fault-injection")]
    if std::env::var("BOXD_FAILPOINT").as_deref() == Ok(_name) {
        std::process::exit(86);
    }
}

impl Runtime {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_isolation(root, None)
    }

    pub fn open_with_isolation(root: &Path, config: Option<&Path>) -> Result<Self> {
        let requested = config.map(Config::load).transpose()?;
        let persisted = root.join("isolation.json");
        let stored = match fs::symlink_metadata(&persisted) {
            Ok(_) => Some(Config::load(&persisted)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let (Some(requested), Some(stored)) = (&requested, &stored)
            && requested != stored
        {
            return Err(Error::Invalid(
                "isolation policy cannot change for an existing state store".into(),
            ));
        }
        let isolation = stored.clone().or(requested);
        if isolation.is_some() {
            if root.exists() {
                isolation::trusted_path(root)?;
            } else {
                isolation::trusted_path(root.parent().ok_or_else(|| {
                    Error::Invalid("state directory needs a trusted parent".into())
                })?)?;
            }
        }
        storage::private_dir(root)?;
        let root = fs::canonicalize(root)?;
        for child in ["boxes", "snapshots"] {
            storage::private_dir(&root.join(child))?;
        }
        if let Some(config) = &isolation
            && stored.is_none()
        {
            let _lock = storage::lock(&root)?;
            if root.join("isolation.json").try_exists()? {
                return Err(Error::Invalid(
                    "isolation policy was initialized concurrently; reopen the store".into(),
                ));
            }
            if fs::read_dir(root.join("boxes"))?.next().is_some()
                || fs::read_dir(root.join("snapshots"))?.next().is_some()
            {
                return Err(Error::Invalid(
                    "isolated profile requires an empty, dedicated state store".into(),
                ));
            }
            config.preflight()?;
            storage::atomic_json(&root.join("isolation.json"), config)?;
        }
        Ok(Self { root, isolation })
    }

    pub fn is_isolated(&self) -> bool {
        self.isolation.is_some()
    }

    fn allocate_identity(&self, records: &[BoxRecord]) -> Result<Option<JailIdentity>> {
        self.isolation
            .as_ref()
            .map(|config| {
                config.allocate(
                    &records
                        .iter()
                        .filter_map(|r| r.jail.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .transpose()
    }

    fn directory(&self, id: &str) -> Result<PathBuf> {
        if !storage::valid_id(id) {
            return Err(Error::Invalid("invalid box ID".into()));
        }
        let directory = self.root.join("boxes").join(id);
        if !directory.exists() {
            return Err(Error::Invalid("box not found".into()));
        }
        storage::private_dir(&directory)?;
        Ok(directory)
    }

    fn generation(&self, record: &BoxRecord) -> Result<PathBuf> {
        if !storage::valid_id(&record.run) {
            return Err(Error::Invalid("invalid run generation".into()));
        }
        let path = self.directory(&record.id)?.join(&record.run);
        storage::private_dir(&path)?;
        Ok(path)
    }

    fn run(&self, record: &BoxRecord) -> Result<PathBuf> {
        let mut path = self.generation(record)?;
        if let Some(identity) = &record.jail {
            for name in ["firecracker", &record.run] {
                path.push(name);
                storage::private_dir(&path)?;
            }
            path.push("root");
            if path.try_exists()? {
                use std::os::unix::fs::MetadataExt;
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_dir()
                    || metadata.mode() & 0o077 != 0
                    || (metadata.uid() != 0 && metadata.uid() != identity.uid)
                {
                    return Err(Error::Invalid("invalid jail root".into()));
                }
            } else {
                storage::private_dir(&path)?;
            }
        }
        Ok(path)
    }

    fn save(&self, record: &BoxRecord) -> Result<()> {
        storage::atomic_json(&self.directory(&record.id)?.join("box.json"), record)
    }

    fn read(&self, id: &str) -> Result<BoxRecord> {
        let record: BoxRecord = storage::read_json(&self.directory(id)?.join("box.json"))?;
        if record.id != id || record.schema_version != 1 {
            return Err(Error::Invalid("invalid box record".into()));
        }
        match (&self.isolation, &record.jail) {
            (Some(config), Some(identity)) => config.validate_identity(identity)?,
            (None, None) => (),
            _ => {
                return Err(Error::Invalid(
                    "box isolation policy mismatch; refusing unjailed fallback".into(),
                ));
            }
        }
        Ok(record)
    }

    async fn allocation(&self) -> Result<File> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match storage::lock(&self.root) {
                Err(Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                Ok(lock) => {
                    if self.root.join("isolation.json").try_exists()? != self.is_isolated() {
                        return Err(Error::Invalid(
                            "state store isolation policy changed; reopen the store".into(),
                        ));
                    }
                    return Ok(lock);
                }
                result => return result,
            }
        }
    }

    fn records(&self) -> Result<Vec<BoxRecord>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(self.root.join("boxes"))? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if storage::valid_id(&name) {
                records.push(self.read(&name)?);
            }
        }
        Ok(records)
    }

    fn connection(&self, record: &BoxRecord) -> Result<(File, Client)> {
        let directory = File::open(self.run(record)?)?;
        let socket = PathBuf::from(format!("/proc/self/fd/{}/api.sock", directory.as_raw_fd()));
        let client = Client::new(&socket, Duration::from_secs(30))?;
        Ok((directory, client))
    }

    async fn guest(
        &self,
        record: &BoxRecord,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response> {
        let directory = File::open(self.run(record)?)?;
        guest::request(
            Path::new(&format!(
                "/proc/self/fd/{}/vsock.sock",
                directory.as_raw_fd()
            )),
            request,
            timeout,
        )
        .await
    }

    fn identify_process(&self, record: &mut BoxRecord) -> Result<()> {
        let run = self.run(record)?;
        let executable = if self.is_isolated() {
            run.join("firecracker")
        } else {
            host::executable("firecracker")?
        };
        let mut executables = vec![executable.as_path()];
        if let Some(config) = &self.isolation {
            executables.push(&config.jailer);
        }
        let identity = match &record.process {
            Some(expected) => match process::identify(expected.pid) {
                Ok(actual)
                    if process::same_launch(
                        expected,
                        &actual,
                        self.isolation.as_ref().map(|_| executable.as_path()),
                    ) && actual.matches_paths(&run, &executables)? =>
                {
                    Some(actual)
                }
                Ok(_) => {
                    return Err(Error::Invalid(
                        "recorded PID now belongs to a different process".into(),
                    ));
                }
                Err(_) => process::find(&run, &executables)?,
            },
            None => process::find(&run, &executables)?,
        };
        record.process = identity;
        Ok(())
    }

    fn terminate(&self, record: &mut BoxRecord) -> Result<()> {
        self.identify_process(record)?;
        if let Some(identity) = &record.process {
            let successor = self
                .isolation
                .as_ref()
                .map(|_| self.run(record).map(|p| p.join("firecracker")))
                .transpose()?;
            process::terminate_with_successor(identity, successor.as_deref())?;
        }
        record.process = None;
        if let Some(config) = &self.isolation {
            config.cleanup_cgroup(&record.run)?;
        }
        Ok(())
    }

    fn verify_isolation(&self, record: &BoxRecord) -> Result<()> {
        if let (Some(config), Some(identity), Some(process)) =
            (&self.isolation, &record.jail, &record.process)
        {
            config.verify(
                process.pid,
                &record.run,
                &self.run(record)?,
                identity,
                record.memory_mib,
                record.vcpus,
            )?;
        }
        Ok(())
    }

    async fn observed(&self, record: &mut BoxRecord) -> Result<()> {
        self.identify_process(record)?;
        if record.process.is_none() {
            record.state = "stopped".into();
            return Ok(());
        }
        let (_directory, client) = self.connection(record)?;
        let status = client.request("GET", "/", Value::Null).await?;
        record.state = match status["state"].as_str() {
            Some("Running") => "running",
            Some("Paused") => "paused",
            _ => "starting",
        }
        .into();
        if matches!(record.state.as_str(), "running" | "paused") {
            self.verify_isolation(record)?;
        }
        Ok(())
    }

    pub async fn inspect(&self, id: &str) -> Result<BoxRecord> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        let interrupted_launch = matches!(
            record.operation.as_deref(),
            Some("create" | "clone" | "start" | "restore")
        );
        if let Err(error) = self.observed(&mut record).await {
            if !interrupted_launch {
                return Err(error);
            }
            // The manager can die while jailer is still setting up, before
            // there is an API to query. Termination still requires PID ownership.
            self.terminate(&mut record)?;
            record.state = "stopped".into();
            record.operation = None;
            record.last_error = Some(format!("interrupted launch stopped: {error}"));
            self.save(&record)?;
            return Ok(record);
        }
        if interrupted_launch {
            // A crash before API configuration leaves a real but unusable VMM.
            // Stop only that identified process; retain its disk for explicit
            // start/retry. Adopt a fully initialized running VM instead.
            let ready = record.state == "running"
                && matches!(
                    self.guest(
                        &record,
                        &Request::Hello {
                            version: PROTOCOL_VERSION
                        },
                        Duration::from_secs(2)
                    )
                    .await,
                    Ok(Response::Hello {
                        initialized: true,
                        ..
                    })
                );
            if !ready {
                self.terminate(&mut record)?;
                record.state = "stopped".into();
            }
            record.operation = None;
            record.last_error = Some(
                if ready {
                    "adopted initialized VM after interrupted launch"
                } else {
                    "interrupted launch stopped; disk retained for explicit start or restore"
                }
                .into(),
            );
        }
        // Interrupted captures leave an explicit intent. Recovery restores the
        // original running state, but never publishes partial snapshot files.
        if record.operation.as_deref() == Some("capture-running") && record.state == "paused" {
            let (_directory, client) = self.connection(&record)?;
            client
                .request("PATCH", "/vm", json!({"state":"Resumed"}))
                .await?;
            record.state = "running".into();
            record.last_error = Some(
                "recovered interrupted checkpoint; incomplete artifacts were not published".into(),
            );
            record.operation = None;
        }
        self.save(&record)?;
        Ok(record)
    }

    pub async fn list(&self) -> Result<Vec<BoxRecord>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(self.root.join("boxes"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if storage::valid_id(&name) {
                records.push(self.inspect(&name).await?);
            }
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(records)
    }

    pub async fn create(
        &self,
        manifest: &Path,
        name: &str,
        memory_mib: u32,
        vcpus: u8,
        template: bool,
    ) -> Result<BoxRecord> {
        let phase = Phase::start("image_verification");
        if name.is_empty()
            || name.len() > 63
            || !(128..=2048).contains(&memory_mib)
            || !(1..=4).contains(&vcpus)
        {
            return Err(Error::Invalid(
                "name must be 1..63 bytes, memory 128..2048 MiB, vCPUs 1..4".into(),
            ));
        }
        if let Some(config) = &self.isolation {
            config.preflight()?;
        }
        let report = host::check();
        if !report.development_ready {
            return Err(Error::Invalid(report.problems.join("; ")));
        }
        let image = image::load(manifest)?;
        if self.is_isolated() {
            isolation::trusted_path(&fs::canonicalize(manifest)?)?;
            isolation::trusted_path(&image.kernel_path)?;
            isolation::trusted_path(&image.rootfs_path)?;
        }
        drop(phase);
        let phase = Phase::start("allocation_wait");
        let allocation = self.allocation().await?;
        drop(phase);
        let phase = Phase::start("reservation");
        let records = self.records()?;
        if records.len() >= 8
            || records.iter().map(|r| r.memory_mib).sum::<u32>() + memory_mib > 8192
        {
            return Err(Error::Invalid(
                "local quota exceeded (8 boxes / 8192 MiB allocated)".into(),
            ));
        }
        let mut record = BoxRecord {
            schema_version: 1,
            id: id()?,
            name: name.into(),
            state: "creating".into(),
            run: id()?,
            process: None,
            image,
            host: host::fingerprint()?,
            memory_mib,
            vcpus,
            source: None,
            operation: Some("create".into()),
            last_error: None,
            disk_copy: String::new(),
            jail: self.allocate_identity(&records)?,
        };
        storage::private_dir(&self.root.join("boxes").join(&record.id))?;
        let _lock = storage::lock(&self.directory(&record.id)?)?;
        let run = self.run(&record)?;
        self.save(&record)?;
        drop(allocation); // The durable record reserves quota before parallel I/O.
        drop(phase);
        let result = async {
            let phase = Phase::start("disk_copy");
            let method = storage::copy_disk(
                &record.image.rootfs_path,
                &run.join("disk.ext4"),
                Instant::now() + Duration::from_secs(120),
            )?;
            record.disk_copy = format!("{method:?}");
            drop(phase);
            self.boot(&mut record, None, !template).await
        }
        .await;
        self.finish_launch(&mut record, result)?;
        Ok(record)
    }

    fn finish_launch(&self, record: &mut BoxRecord, result: Result<()>) -> Result<()> {
        let _phase = Phase::start("launch_commit");
        match result {
            Ok(()) => {
                record.operation = None;
                record.last_error = None;
                self.save(record)
            }
            Err(error) => {
                record.state = "failed".into();
                record.last_error = Some(error.to_string());
                self.terminate(record)?;
                self.save(record)?;
                Err(error)
            }
        }
    }

    async fn boot(
        &self,
        record: &mut BoxRecord,
        snapshot: Option<&Snapshot>,
        initialize: bool,
    ) -> Result<()> {
        let phase = Phase::start("boot_preflight");
        if let Some(config) = &self.isolation {
            config.preflight()?;
        }
        let report = host::check();
        if !report.development_ready || record.host != host::fingerprint()? {
            return Err(Error::Invalid(
                "host or VMM changed; refusing incompatible launch".into(),
            ));
        }
        drop(phase);
        let phase = Phase::start("vmm_start");
        let run = self.run(record)?;
        let mut kernel = record.image.kernel_path.clone();
        let mut snapshot_directory = snapshot.map(|s| self.root.join("snapshots").join(&s.id));
        let mut command = if let (Some(config), Some(identity)) = (&self.isolation, &record.jail) {
            if run.join("firecracker").try_exists()? {
                return Err(Error::Invalid("refusing to reuse a jail generation".into()));
            }
            isolation::own_file(&run.join("disk.ext4"), identity)?;
            if let Some(snapshot) = snapshot {
                for name in ["state.snap", "memory.snap"] {
                    let source = self.root.join("snapshots").join(&snapshot.id).join(name);
                    let hash = snapshot
                        .hashes
                        .get(name)
                        .ok_or_else(|| Error::Invalid("snapshot hash missing".into()))?;
                    isolation::stage_readonly(&source, &run.join(name), hash)?;
                }
                snapshot_directory = Some(PathBuf::from("."));
            } else {
                isolation::trusted_path(&record.image.kernel_path)?;
                isolation::stage_readonly(
                    &record.image.kernel_path,
                    &run.join("kernel"),
                    &record.image.kernel_sha256,
                )?;
                kernel = "kernel".into();
            }
            fs::create_dir(config.cgroup(&record.run)?)?;
            let mut command = Command::new(&config.jailer);
            command.args(config.args(
                &self.generation(record)?,
                &record.run,
                identity,
                record.memory_mib,
                record.vcpus,
            )?);
            command.env_clear();
            command
        } else {
            let mut command = Command::new(host::executable("firecracker")?);
            command.args(["--id", &record.id, "--api-sock", "api.sock"]);
            command
        };
        for socket in ["api.sock", "vsock.sock"] {
            match fs::remove_file(run.join(socket)) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        let log = self.generation(record)?.join("console.log");
        let output = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&log)?;
        command
            .current_dir(&run)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output);
        // SAFETY: only async-signal-safe syscalls before exec. The file budget
        // must accommodate full memory snapshots (up to 2 GiB), as well as logs.
        let isolated = self.is_isolated();
        unsafe {
            command.pre_exec(move || {
                libc::umask(0o077);
                if isolated && libc::setgroups(0, std::ptr::null()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let limit = libc::rlimit {
                    rlim_cur: 3 * 1024 * 1024 * 1024,
                    rlim_max: 3 * 1024 * 1024 * 1024,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        failpoint("before-spawn");
        let mut child = command.spawn()?;
        let pid = child.id();
        failpoint("after-spawn");
        record.process = Some(process::identify(pid)?);
        self.save(record)?;
        failpoint("after-process-record");
        // The VM outlives short CLI invocations. Long-lived callers (including
        // benchmarks) still reap children when another operation stops them.
        std::thread::Builder::new()
            .name("vmm-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        let (_directory, client) = self.connection(record)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if client.request("GET", "/", Value::Null).await.is_ok() {
                break;
            }
            if Instant::now() >= deadline || process::identify(pid).is_err() {
                return Err(Error::Invalid(format!(
                    "VMM startup failed; see {}",
                    log.display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.identify_process(record)?;
        self.save(record)?;
        drop(phase);
        let phase = Phase::start("vmm_configure");
        if let Some(directory) = snapshot_directory {
            client.request("PUT", "/snapshot/load", json!({"snapshot_path":directory.join("state.snap"), "mem_backend":{"backend_type":"File", "backend_path": directory.join("memory.snap")}, "resume_vm":false, "track_dirty_pages":false})).await?;
            client
                .request("PATCH", "/vm", json!({"state":"Resumed"}))
                .await?;
        } else {
            if image::sha256(&run.join(&kernel))? != record.image.kernel_sha256 {
                return Err(Error::Invalid("kernel checksum changed".into()));
            }
            client
                .request(
                    "PUT",
                    "/machine-config",
                    json!({"vcpu_count":record.vcpus,"mem_size_mib":record.memory_mib,"smt":false}),
                )
                .await?;
            client
                .request(
                    "PUT",
                    "/boot-source",
                    json!({"kernel_image_path":kernel,"boot_args":record.image.boot_args()}),
                )
                .await?;
            client.request("PUT", "/drives/rootfs", json!({"drive_id":"rootfs","path_on_host":"disk.ext4","is_root_device":true,"is_read_only":false})).await?;
            client
                .request(
                    "PUT",
                    "/vsock",
                    json!({"guest_cid":3,"uds_path":"vsock.sock"}),
                )
                .await?;
            client
                .request("PUT", "/actions", json!({"action_type":"InstanceStart"}))
                .await?;
        }
        self.verify_isolation(record)?;
        drop(phase);
        let phase = Phase::start("guest_ready");
        let deadline = Instant::now() + Duration::from_secs(20);
        let initialized = loop {
            if let Ok(Response::Hello {
                version: PROTOCOL_VERSION,
                initialized,
            }) = self
                .guest(
                    record,
                    &Request::Hello {
                        version: PROTOCOL_VERSION,
                    },
                    Duration::from_secs(1),
                )
                .await
            {
                break initialized;
            }
            if Instant::now() >= deadline || process::identify(pid).is_err() {
                return Err(Error::Invalid(format!(
                    "guest readiness failed; see {}",
                    log.display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        drop(phase);
        let _phase = Phase::start("guest_initialize");
        if initialize
            && !initialized
            && !matches!(
                self.guest(
                    record,
                    &Request::Initialize(InitializeRequest {
                        hostname: format!("box-{}", &record.id[..12]),
                        machine_id: record.id.clone(),
                        entropy: random_bytes(64)?
                    }),
                    Duration::from_secs(5)
                )
                .await?,
                Response::Initialized {
                    version: PROTOCOL_VERSION
                }
            )
        {
            return Err(Error::Invalid(
                "unexpected guest initialization response".into(),
            ));
        }
        if initialize && record.image.boot_mode == image::BootMode::Systemd {
            // Initialize acknowledges identity provisioning, not systemd startup.
            // The bootstrap closes its listener before that acknowledgement.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if matches!(
                    self.guest(
                        record,
                        &Request::Hello {
                            version: PROTOCOL_VERSION
                        },
                        Duration::from_secs(1)
                    )
                    .await,
                    Ok(Response::Hello {
                        version: PROTOCOL_VERSION,
                        initialized: true
                    })
                ) {
                    break;
                }
                if Instant::now() >= deadline || process::identify(pid).is_err() {
                    return Err(Error::Invalid(format!(
                        "systemd guest agent handoff failed; see {}",
                        log.display()
                    )));
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        record.state = "running".into();
        Ok(())
    }

    pub async fn exec(&self, id: &str, request: ExecRequest) -> Result<ExecResult> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if record.state != "running" {
            return Err(Error::Invalid(format!(
                "cannot execute while {}",
                record.state
            )));
        }
        let timeout = Duration::from_millis(request.timeout_ms.min(3_600_000) + 2000);
        match self
            .guest(&record, &Request::Exec(request), timeout)
            .await?
        {
            Response::Exec(result) => Ok(result),
            _ => Err(Error::Invalid("unexpected guest response".into())),
        }
    }

    pub async fn set_paused(&self, id: &str, paused: bool) -> Result<BoxRecord> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if record.state != "running" && record.state != "paused" {
            return Err(Error::Invalid("box is not running or paused".into()));
        }
        let (_directory, client) = self.connection(&record)?;
        client
            .request(
                "PATCH",
                "/vm",
                json!({"state":if paused {"Paused"} else {"Resumed"}}),
            )
            .await?;
        record.state = if paused { "paused" } else { "running" }.into();
        self.save(&record)?;
        Ok(record)
    }

    pub async fn stop(&self, id: &str, force: bool) -> Result<BoxRecord> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        if force {
            self.identify_process(&mut record)?;
            record.last_error =
                Some("forced stop; guest memory and unflushed writes are lost".into());
        } else {
            self.observed(&mut record).await?;
            if record.state == "paused" {
                let (_fd, client) = self.connection(&record)?;
                client
                    .request("PATCH", "/vm", json!({"state":"Resumed"}))
                    .await?;
                record.state = "running".into();
            }
        }
        if !force
            && record.state == "running"
            && matches!(
                self.guest(
                    &record,
                    &Request::Hello {
                        version: PROTOCOL_VERSION
                    },
                    Duration::from_secs(2)
                )
                .await?,
                Response::Hello {
                    initialized: true,
                    ..
                }
            )
        {
            let response = self
                .guest(
                    &record,
                    &Request::Exec(ExecRequest {
                        argv: vec!["/bin/sync".into()],
                        cwd: None,
                        env: Default::default(),
                        timeout_ms: 5000,
                    }),
                    Duration::from_secs(7),
                )
                .await?;
            if !matches!(
                response,
                Response::Exec(ExecResult {
                    exit_code: Some(0),
                    timed_out: false,
                    ..
                })
            ) {
                return Err(Error::Invalid(
                    "guest sync failed; use stop --force to discard unflushed writes".into(),
                ));
            }
        }
        self.terminate(&mut record)?;
        record.state = "stopped".into();
        record.operation = None;
        self.save(&record)?;
        Ok(record)
    }

    pub async fn start(&self, id: &str) -> Result<BoxRecord> {
        let _allocation = storage::lock(&self.root)?;
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if record.state != "stopped" {
            return Err(Error::Invalid("box must be stopped before starting".into()));
        }
        let previous = record.clone();
        if self.is_isolated() {
            self.terminate(&mut record)?;
            record.run = self::id()?;
            storage::copy_disk(
                &self.run(&previous)?.join("disk.ext4"),
                &self.run(&record)?.join("disk.ext4"),
                Instant::now() + Duration::from_secs(120),
            )?;
        }
        record.operation = Some("start".into());
        self.save(&record)?;
        let result = self.boot(&mut record, None, true).await;
        self.finish_launch(&mut record, result)?;
        if self.is_isolated() {
            fs::remove_dir_all(self.generation(&previous)?)?;
        }
        Ok(record)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let _allocation = storage::lock(&self.root)?;
        // stop obtains the per-box lock; reacquire before removing the record.
        self.stop(id, true).await?;
        let directory = self.directory(id)?;
        let _lock = storage::lock(&directory)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if record.process.is_some() {
            return Err(Error::Invalid("box restarted during deletion".into()));
        }
        fs::remove_dir_all(directory)?;
        File::open(self.root.join("boxes"))?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn interrupted_jailer_without_an_api_is_stopped_not_adopted() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut runtime = Runtime::open(root.path()).unwrap();
        // An ordinary child stands in for a launcher that has not exec'd yet.
        // No jail, cgroup, privilege escalation, or VM is created by this test.
        let uid = unsafe { libc::geteuid() }.max(42);
        let gid = unsafe { libc::getegid() }.max(42);
        runtime.isolation = Some(Config {
            firecracker: "/unused/firecracker".into(),
            jailer: fs::canonicalize("/bin/sleep").unwrap(),
            cgroup_parent: format!("boxd-unit-{}", id().unwrap()),
            uid_base: uid,
            gid_base: gid,
        });
        let mut record: BoxRecord = serde_json::from_value(json!({
            "schema_version":1, "id":id().unwrap(), "run":id().unwrap(),
            "name":"interrupted", "state":"creating", "process":null,
            "image":{"schema_version":1,"architecture":"x86_64","kernel_path":"unused",
                "kernel_sha256":"unused","rootfs_path":"unused","rootfs_sha256":"unused","agent_protocol_version":1},
            "host":"unused", "memory_mib":256, "vcpus":1, "source":null,
            "operation":"create", "last_error":null, "disk_copy":"Copy",
            "jail":{"uid":uid,"gid":gid}
        })).unwrap();
        storage::private_dir(&root.path().join("boxes").join(&record.id)).unwrap();
        let run = runtime.run(&record).unwrap();
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .current_dir(&run)
            .spawn()
            .unwrap();
        record.process = Some(process::identify(child.id()).unwrap());
        runtime.save(&record).unwrap();
        let result = runtime.inspect(&record.id).await;
        let stopped = child.try_wait().unwrap().is_some();
        if !stopped {
            child.kill().unwrap();
        }
        child.wait().unwrap();
        let recovered = result.unwrap();
        assert!(stopped);
        assert_eq!(recovered.state, "stopped");
        assert!(recovered.process.is_none());
        assert!(recovered.operation.is_none());
        assert!(recovered.last_error.unwrap().contains("interrupted launch"));
    }
}
