use crate::{
    Error, Result,
    firecracker::Client,
    guest, host,
    image::{self, Manifest},
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
}

pub struct Runtime {
    root: PathBuf,
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
        storage::private_dir(root)?;
        let root = fs::canonicalize(root)?;
        for child in ["boxes", "snapshots"] {
            storage::private_dir(&root.join(child))?;
        }
        Ok(Self { root })
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

    fn run(&self, record: &BoxRecord) -> Result<PathBuf> {
        if !storage::valid_id(&record.run) {
            return Err(Error::Invalid("invalid run generation".into()));
        }
        let path = self.directory(&record.id)?.join(&record.run);
        storage::private_dir(&path)?;
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
        let executable = host::executable("firecracker")?;
        let identity = match &record.process {
            Some(expected) => match process::identify(expected.pid) {
                Ok(actual) if actual == *expected => Some(actual),
                Ok(_) => {
                    return Err(Error::Invalid(
                        "recorded PID now belongs to a different process".into(),
                    ));
                }
                Err(_) => process::find(&run, &executable)?,
            },
            None => process::find(&run, &executable)?,
        };
        record.process = identity;
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
        Ok(())
    }

    pub async fn inspect(&self, id: &str) -> Result<BoxRecord> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if matches!(
            record.operation.as_deref(),
            Some("create" | "clone" | "start" | "restore")
        ) {
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
                if let Some(identity) = &record.process {
                    process::terminate(identity)?;
                }
                record.process = None;
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
        if name.is_empty()
            || name.len() > 63
            || !(128..=2048).contains(&memory_mib)
            || !(1..=4).contains(&vcpus)
        {
            return Err(Error::Invalid(
                "name must be 1..63 bytes, memory 128..2048 MiB, vCPUs 1..4".into(),
            ));
        }
        let report = host::check();
        if !report.development_ready {
            return Err(Error::Invalid(report.problems.join("; ")));
        }
        let image = image::load(manifest)?;
        let allocation = self.allocation().await?;
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
        };
        storage::private_dir(&self.root.join("boxes").join(&record.id))?;
        let _lock = storage::lock(&self.directory(&record.id)?)?;
        let run = self.run(&record)?;
        self.save(&record)?;
        drop(allocation); // The durable record reserves quota before parallel I/O.
        let result = async {
            let method = storage::copy_disk(
                &record.image.rootfs_path,
                &run.join("disk.ext4"),
                Instant::now() + Duration::from_secs(120),
            )?;
            record.disk_copy = format!("{method:?}");
            self.boot(&mut record, None, !template).await
        }
        .await;
        self.finish_launch(&mut record, result)?;
        Ok(record)
    }

    fn finish_launch(&self, record: &mut BoxRecord, result: Result<()>) -> Result<()> {
        match result {
            Ok(()) => {
                record.operation = None;
                record.last_error = None;
                self.save(record)
            }
            Err(error) => {
                record.state = "failed".into();
                record.last_error = Some(error.to_string());
                if let Some(identity) = &record.process
                    && process::identify(identity.pid).is_ok()
                {
                    process::terminate(identity)?;
                }
                record.process = None;
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
        let report = host::check();
        if !report.development_ready || record.host != host::fingerprint()? {
            return Err(Error::Invalid(
                "host or VMM changed; refusing incompatible launch".into(),
            ));
        }
        let run = self.run(record)?;
        for socket in ["api.sock", "vsock.sock"] {
            match fs::remove_file(run.join(socket)) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        let output = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(run.join("console.log"))?;
        let mut command = Command::new(host::executable("firecracker")?);
        command
            .current_dir(&run)
            .args(["--id", &record.id, "--api-sock", "api.sock"])
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output);
        // SAFETY: only async-signal-safe syscalls before exec. The file budget
        // must accommodate full memory snapshots (up to 2 GiB), as well as logs.
        unsafe {
            command.pre_exec(|| {
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
                    run.join("console.log").display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if let Some(snapshot) = snapshot {
            let directory = self.root.join("snapshots").join(&snapshot.id);
            client.request("PUT", "/snapshot/load", json!({"snapshot_path":directory.join("state.snap"), "mem_backend":{"backend_type":"File", "backend_path": directory.join("memory.snap")}, "resume_vm":false, "track_dirty_pages":false})).await?;
            client
                .request("PATCH", "/vm", json!({"state":"Resumed"}))
                .await?;
        } else {
            if image::sha256(&record.image.kernel_path)? != record.image.kernel_sha256 {
                return Err(Error::Invalid("kernel checksum changed".into()));
            }
            client
                .request(
                    "PUT",
                    "/machine-config",
                    json!({"vcpu_count":record.vcpus,"mem_size_mib":record.memory_mib,"smt":false}),
                )
                .await?;
            client.request("PUT", "/boot-source", json!({"kernel_image_path":record.image.kernel_path,"boot_args":"console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/init quiet"})).await?;
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
                    run.join("console.log").display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
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
        if let Some(identity) = &record.process {
            process::terminate(identity)?;
        }
        record.process = None;
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
        record.operation = Some("start".into());
        self.save(&record)?;
        let result = self.boot(&mut record, None, true).await;
        self.finish_launch(&mut record, result)?;
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
