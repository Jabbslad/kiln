use crate::{
    Failure,
    journal::{Conflict, Journal, JournalFull},
};
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use kiln_api::{
    Action, ApiError, BoxView, CheckpointView, Operation, OperationState, OperationSubmitQuery,
    Outcome, Submit, TemplateView, valid_alias, valid_id,
};
use kiln_runtime::{
    isolation,
    runtime::{BoxRecord, Runtime},
    storage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{net::UnixListener, sync::Semaphore};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub runtime_dir: PathBuf,
    pub journal_dir: PathBuf,
    pub socket: PathBuf,
    pub isolation_config: Option<PathBuf>,
    #[serde(default)]
    pub allow_unsafe_development: bool,
    /// Unassigned jails kept ready for template create. Zero leaves create cold.
    #[serde(default)]
    pub jail_pool: u8,
    pub templates: BTreeMap<String, String>,
}

pub struct Host {
    runtime: Runtime,
    jail_pool: u8,
    runtime_dir: PathBuf,
    templates: BTreeMap<String, String>,
    catalog: Vec<TemplateView>,
    journal: Mutex<Journal>,
    capacity: Arc<Semaphore>,
    sessions: Arc<Semaphore>,
    metrics: Mutex<crate::metrics::Sampler>,
    _locks: [File; 2],
}

fn acquire_service_lock(file: &File) -> Result<()> {
    let mut pause = std::time::Duration::from_millis(1);
    for _ in 0..6 {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(pause);
                pause *= 2;
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::Error(error) => error.into(),
        std::fs::TryLockError::WouldBlock => {
            std::io::Error::from(std::io::ErrorKind::WouldBlock).into()
        }
    })
}

impl Host {
    pub fn open(config: &Config) -> Result<Arc<Self>> {
        ensure!(config.jail_pool <= 8, "jail pool size must be 0..=8");
        if unsafe { libc::geteuid() } == 0 {
            ensure!(
                !config.allow_unsafe_development,
                "root service cannot enable unsafe development"
            );
            for path in [&config.runtime_dir, &config.journal_dir] {
                isolation::trusted_path(if path.exists() {
                    path
                } else {
                    path.parent()
                        .ok_or_else(|| anyhow::anyhow!("state needs a parent"))?
                })?;
            }
        }
        let runtime =
            Runtime::open_with_isolation(&config.runtime_dir, config.isolation_config.as_deref())?;
        ensure!(
            runtime.is_isolated() || config.allow_unsafe_development,
            "isolated runtime required; development needs explicit allow_unsafe_development"
        );
        ensure!(
            !(runtime.is_isolated() && config.allow_unsafe_development),
            "development flag conflicts with isolated state"
        );
        if !runtime.is_isolated() {
            eprintln!(
                "WARNING: unsafe development runtime; trusted workloads only, no guest network"
            );
        }
        let runtime_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(config.runtime_dir.join(".service.lock"))?;
        // flock can return EAGAIN just after another thread in this process
        // closes a different lock file. A holder that is still alive still fails.
        acquire_service_lock(&runtime_lock)?;
        storage::private_dir(&config.journal_dir)?;
        let journal_lock = storage::lock(&config.journal_dir)?;
        let snapshots = runtime.snapshots()?;
        ensure!(
            config.templates.len() <= 16,
            "at most 16 published templates"
        );
        let mut catalog = Vec::new();
        for (name, id) in &config.templates {
            ensure!(
                valid_alias(name) && valid_id(id),
                "invalid catalog alias or template ID"
            );
            let snapshot = snapshots
                .iter()
                .find(|s| &s.id == id && s.template)
                .ok_or_else(|| {
                    anyhow::anyhow!("catalog references a missing prepared template: {name}")
                })?;
            ensure!(
                snapshot.isolated == runtime.is_isolated(),
                "template isolation mismatch"
            );
            catalog.push(TemplateView {
                name: name.clone(),
                memory_mib: snapshot.memory_mib,
                vcpus: snapshot.vcpus,
            });
        }
        let journal = Journal::open(&config.journal_dir.join("operations.sqlite"))?;
        let mut metrics =
            crate::metrics::Sampler::new(crate::metrics::Paths::live(config.runtime_dir.clone()));
        metrics.prime();
        Ok(Arc::new(Self {
            runtime,
            jail_pool: config.jail_pool,
            runtime_dir: config.runtime_dir.clone(),
            templates: config.templates.clone(),
            catalog,
            journal: Mutex::new(journal),
            capacity: Arc::new(Semaphore::new(8)),
            sessions: Arc::new(Semaphore::new(32)),
            metrics: Mutex::new(metrics),
            _locks: [runtime_lock, journal_lock],
        }))
    }

    /// Reap leftover unclaimed jails, then keep `jail_pool` generations ready
    /// when every published template shares one shape. `--check` does not call
    /// this. A later start reaps anything shutdown did not reach.
    pub fn arm_jail_pool(&self) -> Result<()> {
        if self.jail_pool == 0 || !self.runtime.is_isolated() {
            if self.jail_pool > 0 {
                anyhow::bail!("jail pool requires an isolated runtime");
            }
            self.runtime.arm_jail_pool(0, None)?;
            return Ok(());
        }
        let snapshots = self.runtime.snapshots()?;
        let mut shape = None;
        for id in self.templates.values() {
            let snapshot = snapshots
                .iter()
                .find(|item| &item.id == id)
                .ok_or_else(|| anyhow::anyhow!("catalog references a missing prepared template"))?;
            let next = (
                snapshot.memory_mib,
                snapshot.vcpus,
                snapshot.network.is_some(),
            );
            if let Some(current) = shape {
                if current != next {
                    eprintln!(
                        "jail pool disabled: published templates do not share one memory, vcpu, and network shape"
                    );
                    self.runtime.arm_jail_pool(0, None)?;
                    return Ok(());
                }
            } else {
                shape = Some(next);
            }
        }
        let Some((memory_mib, vcpus, networked)) = shape else {
            self.runtime.arm_jail_pool(0, None)?;
            return Ok(());
        };
        self.runtime
            .arm_jail_pool(self.jail_pool, Some((memory_mib, vcpus, networked)))?;
        Ok(())
    }

    pub fn shutdown_jail_pool(&self) -> Result<()> {
        self.runtime.shutdown_jail_pool()?;
        Ok(())
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/api/v1/dashboard", get(dashboard))
            .route("/v1/templates", get(templates))
            .route("/v1/boxes", get(boxes))
            .route("/v1/boxes/{id}", get(inspect))
            .route("/v1/boxes/{id}/ssh-key", get(ssh_key))
            .route("/v1/boxes/{id}/ssh", get(ssh_tunnel))
            .route("/v1/operations/{id}", get(operation))
            .route("/v1/operations", post(submit))
            .layer(DefaultBodyLimit::max(kiln_api::MAX_REQUEST_BYTES))
            .with_state(self.clone())
    }

    fn owns(&self, id: &str) -> Result<(), Failure> {
        if !valid_id(id)
            || !self
                .journal
                .lock()
                .unwrap()
                .owns(id)
                .map_err(Failure::internal)?
        {
            return Err(Failure::missing());
        }
        Ok(())
    }

    fn box_on_disk(&self, box_id: &str) -> Result<bool, Failure> {
        if !valid_id(box_id) {
            return Ok(false);
        }
        self.runtime_dir
            .join("boxes")
            .join(box_id)
            .join("box.json")
            .try_exists()
            .map_err(Failure::internal)
    }

    async fn create_observed_running(&self, box_id: &str) -> Result<bool, Failure> {
        if !self.box_on_disk(box_id)? {
            return Ok(false);
        }
        let record = self
            .runtime
            .inspect(box_id)
            .await
            .map_err(Failure::internal)?;
        Ok(record.state == "running")
    }

    async fn execute(&self, action: Action, box_id: &str) -> kiln_runtime::Result<Outcome> {
        let record = match action {
            Action::Create { template, name } => {
                self.runtime
                    .clone_template_with_id(&self.templates[&template], &name, box_id)
                    .await?
            }
            Action::Pause { id } => self.runtime.set_paused(&id, true).await?,
            Action::Resume { id } => self.runtime.set_paused(&id, false).await?,
            Action::Stop { id, force } => self.runtime.stop(&id, force).await?,
            Action::Start { id } => self.runtime.start(&id).await?,
            Action::Delete { id } => {
                self.runtime.delete(&id).await?;
                return Ok(Outcome::Deleted { id });
            }
            Action::Exec { id, request } => {
                return self.runtime.exec(&id, request).await.map(Outcome::Exec);
            }
            Action::Checkpoint { id } => {
                let snapshot = self.runtime.checkpoint(&id).await?;
                return Ok(Outcome::Checkpoint(CheckpointView {
                    id: snapshot.id,
                    box_id: snapshot.source_box,
                }));
            }
            Action::Restore {
                id,
                checkpoint,
                acknowledge_external_state_replay,
            } => {
                if !acknowledge_external_state_replay {
                    return Err(kiln_runtime::Error::Invalid(
                        "restore rewinds memory and disk, not external systems; pass --acknowledge-external-state-replay"
                            .into(),
                    ));
                }
                self.runtime.restore(&id, &checkpoint).await?
            }
        };
        Ok(Outcome::Box(view(record)))
    }

    fn dashboard(&self) -> Result<crate::dashboard::Document> {
        let journal = self.journal.lock().unwrap();
        let ids = journal.managed_ids()?;
        let created = journal.create_times()?;
        let rows = journal.recent(100)?;
        let mut launch_timings = BTreeMap::new();
        for id in &ids {
            launch_timings.insert(id.clone(), journal.last_launch_ms(id)?);
        }
        drop(journal);
        let mut names = BTreeMap::new();
        let mut boxes = Vec::new();
        let mut sampler = self.metrics.lock().unwrap();
        for id in ids {
            let Some(record) = read_box(&self.runtime_dir, &id)? else {
                continue;
            };
            if record.id != id || record.name.is_empty() {
                continue;
            }
            let Some(created_at) = created.get(&id).copied() else {
                continue;
            };
            let disk_path = box_disk(&self.runtime_dir, &record);
            let disk_bytes = disk_path.clone().and_then(file_len);
            names.insert(id.clone(), record.name.clone());
            let active = matches!(record.state.as_str(), "running" | "paused");
            let cgroup = self.runtime.box_cgroup(&record.run);
            let metrics = sampler.box_metrics(cgroup.as_deref(), disk_path.as_deref(), active);
            let timing = record.last_launch.clone().or_else(|| {
                launch_timings
                    .get(&id)
                    .cloned()
                    .flatten()
                    .map(|(method, total_ms)| {
                        kiln_runtime::launch_timing::LaunchTiming::from_journal(
                            &method,
                            &record.disk_copy,
                            total_ms,
                        )
                    })
            });
            boxes.push(crate::dashboard::BoxFacts {
                id,
                name: record.name,
                owner: record.owner.clone(),
                state: record.state,
                memory_mib: record.memory_mib,
                vcpus: record.vcpus,
                disk_bytes,
                created_at: crate::journal::journal_stored_secs(created_at),
                last_operation: record.operation,
                last_error: record.last_error,
                restored_from: record.source,
                metrics,
                timing,
            });
        }
        let mut operations = Vec::new();
        for row in rows {
            let box_name = names
                .get(&row.box_id)
                .cloned()
                .unwrap_or_else(|| row.box_id.clone());
            operations.push(crate::dashboard::OperationFacts {
                id: row.id,
                box_id: row.box_id,
                box_name,
                is_create: row.is_create,
                created_at: row.created_at,
                operation: row.operation,
            });
        }
        let mut templates = Vec::new();
        for (name, id) in &self.templates {
            let Some(view) = self.catalog.iter().find(|template| &template.name == name) else {
                continue;
            };
            templates.push(crate::dashboard::TemplateFacts {
                id: id.clone(),
                name: name.clone(),
                memory_mib: view.memory_mib,
                vcpus: view.vcpus,
                development_only: !self.runtime.is_isolated(),
                disk_bytes: snapshot_disk(&self.runtime_dir, id).and_then(file_len),
            });
        }
        let isolated = self.runtime.is_isolated();
        let host_fingerprint = kiln_runtime::host::fingerprint().ok();
        let mut checkpoints = Vec::new();
        for snapshot in self.runtime.snapshots()? {
            if snapshot.template || !valid_id(&snapshot.id) || !valid_id(&snapshot.source_box) {
                continue;
            }
            checkpoints.push(crate::dashboard::CheckpointFacts {
                id: snapshot.id.clone(),
                box_id: snapshot.source_box,
                created_at: snapshot_created(&self.runtime_dir, &snapshot.id).unwrap_or(0),
                size_bytes: checkpoint_bytes(&self.runtime_dir, &snapshot.id),
                compatible: snapshot.schema_version == 1
                    && snapshot.isolated == isolated
                    && host_fingerprint.as_deref() == Some(snapshot.host.as_str()),
            });
        }
        checkpoints.sort_by(|left, right| left.id.cmp(&right.id));
        let host_metrics = sampler.host();
        Ok(crate::dashboard::document(
            "",
            &crate::dashboard::utc_timestamp(crate::dashboard::unix_now()),
            "online",
            host_metrics,
            &crate::dashboard::Inventory {
                boxes,
                templates,
                operations,
                checkpoints,
            },
        ))
    }
}

fn live_box_ids(runtime_dir: &FsPath) -> Result<Vec<String>> {
    let boxes_dir = runtime_dir.join("boxes");
    let Ok(entries) = fs::read_dir(&boxes_dir) else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if kiln_runtime::storage::valid_id(&id) && path.join("box.json").is_file() {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

fn read_box(runtime_dir: &FsPath, id: &str) -> Result<Option<BoxRecord>> {
    if !kiln_runtime::storage::valid_id(id) {
        return Ok(None);
    }
    let path = runtime_dir.join("boxes").join(id).join("box.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn box_disk(runtime_dir: &FsPath, record: &BoxRecord) -> Option<PathBuf> {
    if !kiln_runtime::storage::valid_id(&record.id) || !kiln_runtime::storage::valid_id(&record.run)
    {
        return None;
    }
    let mut path = runtime_dir.join("boxes").join(&record.id).join(&record.run);
    if record.jail.is_some() {
        path.push("firecracker");
        path.push(&record.run);
        path.push("root");
    }
    path.push("disk.ext4");
    Some(path)
}

fn snapshot_disk(runtime_dir: &FsPath, id: &str) -> Option<PathBuf> {
    if !kiln_runtime::storage::valid_id(id) {
        return None;
    }
    Some(runtime_dir.join("snapshots").join(id).join("disk.ext4"))
}

fn snapshot_created(runtime_dir: &FsPath, id: &str) -> Option<i64> {
    if !kiln_runtime::storage::valid_id(id) {
        return None;
    }
    let metadata =
        fs::symlink_metadata(runtime_dir.join("snapshots").join(id).join("snapshot.json")).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    let secs = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(secs).ok()
}

fn checkpoint_bytes(runtime_dir: &FsPath, id: &str) -> Option<u64> {
    if !kiln_runtime::storage::valid_id(id) {
        return None;
    }
    let directory = runtime_dir.join("snapshots").join(id);
    let mut total = 0u64;
    let mut any = false;
    for name in ["disk.ext4", "state.snap", "memory.snap"] {
        let Some(len) = file_len(directory.join(name)) else {
            continue;
        };
        any = true;
        total = total.checked_add(len)?;
    }
    any.then_some(total)
}

fn file_len(path: PathBuf) -> Option<u64> {
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_file() {
        Some(metadata.len())
    } else {
        None
    }
}

fn view(record: BoxRecord) -> BoxView {
    BoxView {
        id: record.id,
        name: record.name,
        state: record.state,
        memory_mib: record.memory_mib,
        vcpus: record.vcpus,
        owner: record.owner,
    }
}

async fn dashboard(
    State(host): State<Arc<Host>>,
) -> Result<Json<crate::dashboard::Document>, Failure> {
    host.dashboard().map(Json).map_err(Failure::internal)
}

async fn templates(State(host): State<Arc<Host>>) -> Json<Vec<TemplateView>> {
    Json(host.catalog.clone())
}

async fn ssh_key(
    State(host): State<Arc<Host>>,
    Path(id): Path<String>,
) -> Result<Json<kiln_api::SshReady>, Failure> {
    host.owns(&id)?;
    host.runtime
        .ssh_host_key(&id)
        .await
        .map(Json)
        .map_err(Failure::internal)
}

async fn ssh_tunnel(
    State(host): State<Arc<Host>>,
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> Result<axum::response::Response, Failure> {
    host.owns(&id)?;
    let key = crate::ssh::admission(&request)?;
    let permit = host
        .sessions
        .clone()
        .try_acquire_owned()
        .map_err(|_| crate::ssh::busy())?;
    let stream = host
        .runtime
        .ssh_connect(&id, &key)
        .await
        .map_err(Failure::internal)?;
    Ok(crate::ssh::upgrade(
        request,
        stream,
        permit,
        crate::revocation::StreamCancel::new(),
    ))
}

async fn boxes(State(host): State<Arc<Host>>) -> Result<Json<Vec<BoxView>>, Failure> {
    let ids = host
        .journal
        .lock()
        .unwrap()
        .managed_ids()
        .map_err(Failure::internal)?;
    let mut records = Vec::new();
    for id in ids {
        if host
            .runtime_dir
            .join("boxes")
            .join(&id)
            .join("box.json")
            .try_exists()
            .map_err(Failure::internal)?
        {
            records.push(view(
                host.runtime.inspect(&id).await.map_err(Failure::internal)?,
            ));
        }
    }
    Ok(Json(records))
}

async fn inspect(
    State(host): State<Arc<Host>>,
    Path(id): Path<String>,
) -> Result<Json<BoxView>, Failure> {
    host.owns(&id)?;
    if !host
        .runtime_dir
        .join("boxes")
        .join(&id)
        .join("box.json")
        .try_exists()
        .map_err(Failure::internal)?
    {
        return Err(Failure::missing());
    }
    Ok(Json(view(
        host.runtime.inspect(&id).await.map_err(Failure::internal)?,
    )))
}

async fn operation(
    State(host): State<Arc<Host>>,
    Path(id): Path<String>,
) -> Result<Json<Operation>, Failure> {
    if !valid_id(&id) {
        return Err(Failure::missing());
    }
    host.journal
        .lock()
        .unwrap()
        .get(&id)
        .map_err(Failure::internal)?
        .map(Json)
        .ok_or_else(Failure::missing)
}

async fn wait_for_create(
    host: Arc<Host>,
    op_id: &str,
    wait_seconds: u64,
) -> Result<Operation, Failure> {
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    let mut interval = Duration::from_millis(50);
    loop {
        let op = host
            .journal
            .lock()
            .unwrap()
            .get(op_id)
            .map_err(Failure::internal)?
            .ok_or_else(Failure::missing)?;
        if op.state != OperationState::Running {
            if matches!(op.state, OperationState::Failed | OperationState::Unknown) {
                return Ok(op);
            }
            if op.state == OperationState::Succeeded
                && host.create_observed_running(&op.box_id).await?
            {
                return Ok(op);
            }
            if Instant::now() >= deadline {
                let message = if op.state == OperationState::Succeeded {
                    "The control plane has not observed the new box."
                } else {
                    "Create did not succeed. The box is shown only after the control plane records it."
                };
                return Err(Failure::new(
                    StatusCode::REQUEST_TIMEOUT,
                    "wait_timeout",
                    message,
                ));
            }
        } else if Instant::now() >= deadline {
            let message = format!(
                "create {op_id} still running after {wait_seconds}s; reconnect with `kiln operation {op_id} --wait`"
            );
            return Err(Failure::new(
                StatusCode::REQUEST_TIMEOUT,
                "wait_timeout",
                &message,
            ));
        }
        tokio::time::sleep(interval).await;
        interval = interval.mul_f64(1.5).min(Duration::from_millis(2_000));
    }
}

fn spawn_operation_worker(
    host: Arc<Host>,
    request: Submit,
    mut op: Operation,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let worker_host = host.clone();
    let journal_host = host;
    let is_exec = matches!(request.action, Action::Exec { .. });
    // Detached from the HTTP request. The outer task records panics as unknown;
    // a process crash is handled by Journal::open, never by command replay.
    tokio::spawn(async move {
        let _permit = permit;
        let box_id = op.box_id.clone();
        let result =
            tokio::spawn(async move { worker_host.execute(request.action, &box_id).await }).await;
        match result {
            Ok(Ok(outcome)) => {
                op.state = OperationState::Succeeded;
                op.result = Some(outcome);
            }
            other => {
                op.state = if is_exec || other.is_err() {
                    OperationState::Unknown
                } else {
                    OperationState::Failed
                };
                op.error = Some(ApiError {
                    code: "runtime_operation_failed".into(),
                    message: "Operation did not return a confirmed result. Inspect the box before issuing another command; consult the host log for lifecycle failures.".into(),
                });
                // Exec inputs and output are never logged, including on error.
                if !is_exec {
                    eprintln!("operation {}: {other:?}", op.id);
                }
            }
        }
        if let Err(error) = journal_host.journal.lock().unwrap().finish(&op) {
            eprintln!("could not persist operation {} outcome: {error}", op.id);
        }
    });
}

async fn finish_create_inline(
    host: &Arc<Host>,
    request: Submit,
    mut op: Operation,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> Operation {
    let box_id = op.box_id.clone();
    let is_exec = matches!(request.action, Action::Exec { .. });
    let result = host.execute(request.action, &box_id).await;
    match result {
        Ok(outcome) => {
            op.state = OperationState::Succeeded;
            op.result = Some(outcome);
        }
        Err(error) => {
            op.state = if is_exec {
                OperationState::Unknown
            } else {
                OperationState::Failed
            };
            op.error = Some(ApiError {
                code: "runtime_operation_failed".into(),
                message: "Operation did not return a confirmed result. Inspect the box before issuing another command; consult the host log for lifecycle failures.".into(),
            });
            eprintln!("operation {}: {error:?}", op.id);
        }
    }
    if let Err(error) = host.journal.lock().unwrap().finish(&op) {
        eprintln!("could not persist operation {} outcome: {error}", op.id);
    }
    op
}

async fn submit_create_wait(
    host: Arc<Host>,
    request: Submit,
    accepted: Operation,
    permit: tokio::sync::OwnedSemaphorePermit,
    wait_seconds: u64,
) -> Result<(StatusCode, Json<Operation>), Failure> {
    let finished = finish_create_inline(&host, request, accepted, permit).await;
    let op = wait_for_create(host, &finished.id, wait_seconds).await?;
    Ok((StatusCode::OK, Json(op)))
}

enum SubmitPlan {
    ReplayExisting(Operation),
    Run {
        request: Submit,
        accepted: Operation,
        permit: tokio::sync::OwnedSemaphorePermit,
    },
}

fn operation_actor(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(kiln_api::OPERATION_ACTOR_HEADER)
        .and_then(|v| v.to_str().ok())?;
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    Some(value)
}

fn plan_submit(
    host: &Arc<Host>,
    request: &Submit,
    actor: Option<&str>,
) -> Result<SubmitPlan, Failure> {
    let mut db = host.journal.lock().unwrap();
    if let Some(existing) = db.existing(request).map_err(|e| {
        if e.is::<Conflict>() {
            Failure::new(
                StatusCode::CONFLICT,
                "id_conflict",
                "Request ID already used for a different action.",
            )
        } else {
            Failure::internal(e)
        }
    })? {
        return Ok(SubmitPlan::ReplayExisting(existing));
    }
    match &request.action {
        Action::Create { template, .. } if !host.templates.contains_key(template) => {
            return Err(Failure::missing());
        }
        action
            if action.box_id().is_some()
                && !db
                    .owns(action.box_id().unwrap())
                    .map_err(Failure::internal)? =>
        {
            return Err(Failure::missing());
        }
        _ => {}
    }
    let permit = host.capacity.clone().try_acquire_owned().map_err(|_| {
        Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Eight operations are already in flight. No new operation was accepted.",
        )
    })?;
    let live_ids = live_box_ids(&host.runtime_dir).map_err(Failure::internal)?;
    let live_refs: Vec<&str> = live_ids.iter().map(String::as_str).collect();
    let accepted = db
        .accept(request, &live_refs, actor)
        .map_err(|error| {
        if error.is::<JournalFull>() {
            Failure::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "journal_full",
                "The operation journal is full after automatic retention. Archive this installation or delete old boxes, then retry.",
            )
        } else {
            Failure::internal(error)
        }
    })?;
    Ok(SubmitPlan::Run {
        request: request.clone(),
        accepted,
        permit,
    })
}

async fn submit(
    State(host): State<Arc<Host>>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    Json(request): Json<Submit>,
) -> Result<(StatusCode, Json<Operation>), Failure> {
    let query = OperationSubmitQuery::parse("/v1/operations", raw_query.as_deref())
        .map_err(|message| Failure::new(StatusCode::BAD_REQUEST, "invalid_request", message))?;
    query
        .validate_for_action(&request.action)
        .map_err(|message| Failure::new(StatusCode::BAD_REQUEST, "invalid_request", message))?;
    if !valid_id(&request.id) {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request ID",
        ));
    }
    request
        .action
        .validate()
        .map_err(|e| Failure::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;
    let plan = plan_submit(&host, &request, operation_actor(&headers))?;
    match plan {
        SubmitPlan::ReplayExisting(existing) => {
            if query.wait {
                let op = wait_for_create(host.clone(), &existing.id, query.wait_seconds).await?;
                return Ok((StatusCode::OK, Json(op)));
            }
            Ok((StatusCode::OK, Json(existing)))
        }
        SubmitPlan::Run {
            request,
            accepted,
            permit,
        } => {
            if query.wait {
                return submit_create_wait(host, request, accepted, permit, query.wait_seconds)
                    .await;
            }
            spawn_operation_worker(host, request, accepted.clone(), permit);
            Ok((StatusCode::ACCEPTED, Json(accepted)))
        }
    }
}

/// Parent directory must be operator-owned and not writable by socket clients.
pub async fn bind_socket(path: &FsPath) -> Result<UnixListener> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("socket needs a parent directory"))?;
    let metadata = fs::symlink_metadata(parent)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o027 == 0,
        "socket directory must be owned by service user, inaccessible to others and not group writable"
    );
    if unsafe { libc::geteuid() } == 0 {
        isolation::trusted_path(parent)?;
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() },
            "refusing to replace non-owned socket path"
        );
        match tokio::net::UnixStream::connect(path).await {
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => fs::remove_file(path)?,
            _ => anyhow::bail!("socket already active or unavailable"),
        }
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use kiln_api::new_id;

    #[test]
    fn jail_pool_defaults_to_zero_and_rejects_a_ninth_slot() {
        let parsed: Config = serde_json::from_value(serde_json::json!({
            "runtime_dir": "/var/lib/kiln/runtime",
            "journal_dir": "/var/lib/kiln/journal",
            "socket": "/run/kiln/host.sock",
            "isolation_config": null,
            "allow_unsafe_development": true,
            "templates": {}
        }))
        .unwrap();
        assert_eq!(parsed.jail_pool, 0);
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let Err(error) = Host::open(&Config {
            runtime_dir: root.path().join("runtime"),
            journal_dir: root.path().join("journal"),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 9,
            templates: Default::default(),
        }) else {
            panic!("jail pool size 9 was accepted");
        };
        assert!(error.to_string().contains("0..=8"), "{error}");
    }

    #[test]
    fn first_dashboard_read_reports_host_cpu() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let host = Host::open(&Config {
            runtime_dir: root.path().join("runtime"),
            journal_dir: root.path().join("journal"),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        })
        .unwrap();
        let value = serde_json::to_value(host.dashboard().unwrap()).unwrap();
        assert!(value["host"]["metrics"]["cpu_percent"].as_f64().is_some());
        assert!(value["host"]["metrics"]["memory_bytes"].as_u64().unwrap() > 0);
        assert!(value["host"]["metrics"]["disk_bytes"].as_u64().unwrap() > 0);
        assert!(
            value["host"]["metrics"]["disk_total_bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
    }

    #[test]
    fn stopped_box_reports_allocated_disk_image() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = root.path().join("runtime");
        let journal = root.path().join("journal");
        let config = Config {
            runtime_dir: runtime.clone(),
            journal_dir: journal.clone(),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        };
        drop(Host::open(&config).unwrap());
        let id: String = "0123456789abcdef0123456789abcdef".into();
        let run: String = "0123456789abcdef0123456789abcde0".into();
        let directory = runtime.join("boxes").join(&id).join(&run);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            runtime.join("boxes").join(&id).join("box.json"),
            serde_json::to_vec(&BoxRecord {
                schema_version: 1,
                id: id.clone(),
                name: "stopped-disk".into(),
                state: "stopped".into(),
                run: run.clone(),
                process: None,
                image: kiln_runtime::image::Manifest {
                    schema_version: 1,
                    architecture: "x86_64".into(),
                    kernel_path: "/kernel".into(),
                    kernel_sha256: "ab".repeat(32),
                    rootfs_path: "/rootfs".into(),
                    rootfs_sha256: "cd".repeat(32),
                    agent_protocol_version: 1,
                    boot_mode: kiln_runtime::image::BootMode::Init,
                },
                host: "fixture-host".into(),
                memory_mib: 256,
                vcpus: 1,
                source: None,
                operation: None,
                last_error: None,
                disk_copy: "Copy".into(),
                disk_layer: None,
                jail: None,
                network: None,
                last_launch: None,
                owner: None,
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(directory.join("disk.ext4"), b"stopped-box-disk").unwrap();
        let host = Host::open(&config).unwrap();
        let db = rusqlite::Connection::open(journal.join("operations.sqlite")).unwrap();
        db.execute(
            "INSERT INTO operations(id, fingerprint, box_id, is_create, running, response, created_at)
             VALUES (?1, ?2, ?3, 1, 0, ?4, ?5)",
            rusqlite::params![
                "0123456789abcdef0123456789abcde1",
                "fixture",
                id,
                r#"{"id":"0123456789abcdef0123456789abcde1","box_id":"0123456789abcdef0123456789abcdef","state":"succeeded","result":{"type":"box","id":"0123456789abcdef0123456789abcdef","name":"stopped-disk","state":"stopped","memory_mib":256,"vcpus":1},"error":null}"#,
                1_700_000_000_i64,
            ],
        )
        .unwrap();
        let value = serde_json::to_value(host.dashboard().unwrap()).unwrap();
        let entry = value["boxes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .expect("stopped box");
        assert_eq!(entry["state"], "stopped");
        assert!(entry["metrics"]["cpu_percent"].is_null());
        assert!(entry["metrics"]["memory_bytes"].is_null());
        assert!(entry["metrics"]["disk_bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn v1_running_box_upgrade_keeps_null_timing_and_passes_self_check() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = root.path().join("runtime");
        let journal = root.path().join("journal");
        fs::create_dir_all(&journal).unwrap();
        fs::set_permissions(&journal, fs::Permissions::from_mode(0o700)).unwrap();
        let id: String = "0123456789abcdef0123456789abcdef".into();
        let run: String = "0123456789abcdef0123456789abcde0".into();
        let response = serde_json::json!({
            "id": "0123456789abcdef0123456789abcde1",
            "box_id": id,
            "state": "succeeded",
            "result": {
                "type": "box",
                "id": id,
                "name": "legacy-running",
                "state": "running",
                "memory_mib": 256,
                "vcpus": 1
            },
            "error": null
        });
        let config = Config {
            runtime_dir: runtime.clone(),
            journal_dir: journal.clone(),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        };
        let conn = rusqlite::Connection::open(journal.join("operations.sqlite")).unwrap();
        conn.execute_batch(
            "PRAGMA user_version=1;
             CREATE TABLE operations (
               id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
               is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
               created_at INTEGER NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at)
             VALUES (?1,?2,?3,1,0,?4,?5)",
            rusqlite::params![
                "0123456789abcdef0123456789abcde1",
                b"fingerprint-bytes",
                id,
                response.to_string(),
                1_700_000_000_i64,
            ],
        )
        .unwrap();
        drop(conn);
        let directory = runtime.join("boxes").join(&id).join(&run);
        fs::create_dir_all(&directory).unwrap();
        for path in [
            &runtime,
            &runtime.join("boxes"),
            &runtime.join("boxes").join(&id),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(
            runtime.join("boxes").join(&id).join("box.json"),
            serde_json::to_vec(&BoxRecord {
                schema_version: 1,
                id: id.clone(),
                name: "legacy-running".into(),
                state: "running".into(),
                run: run.clone(),
                process: None,
                image: kiln_runtime::image::Manifest {
                    schema_version: 1,
                    architecture: "x86_64".into(),
                    kernel_path: "/kernel".into(),
                    kernel_sha256: "ab".repeat(32),
                    rootfs_path: "/rootfs".into(),
                    rootfs_sha256: "cd".repeat(32),
                    agent_protocol_version: 1,
                    boot_mode: kiln_runtime::image::BootMode::Init,
                },
                host: "fixture-host".into(),
                memory_mib: 256,
                vcpus: 1,
                source: None,
                operation: None,
                last_error: None,
                disk_copy: "Copy".into(),
                disk_layer: None,
                jail: None,
                network: None,
                last_launch: None,
                owner: None,
            })
            .unwrap(),
        )
        .unwrap();
        let host = Host::open(&config).unwrap();
        let value = serde_json::to_value(host.dashboard().unwrap()).unwrap();
        assert!(value["boxes"][0]["timing"].is_null());
        crate::dashboard_check::parse_and_validate(&serde_json::to_vec(&value).unwrap())
            .expect("legacy running box stays exempt from the launch gate");
    }

    #[test]
    fn measured_launch_on_box_record_satisfies_self_check() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = root.path().join("runtime");
        let journal = root.path().join("journal");
        let config = Config {
            runtime_dir: runtime.clone(),
            journal_dir: journal.clone(),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        };
        drop(Host::open(&config).unwrap());
        let id: String = "0123456789abcdef0123456789abcdef".into();
        let run: String = "0123456789abcdef0123456789abcde0".into();
        fs::create_dir_all(runtime.join("boxes").join(&id).join(&run)).unwrap();
        fs::write(
            runtime.join("boxes").join(&id).join("box.json"),
            serde_json::to_vec(&BoxRecord {
                schema_version: 1,
                id: id.clone(),
                name: "measured".into(),
                state: "running".into(),
                run: run.clone(),
                process: None,
                image: kiln_runtime::image::Manifest {
                    schema_version: 1,
                    architecture: "x86_64".into(),
                    kernel_path: "/kernel".into(),
                    kernel_sha256: "ab".repeat(32),
                    rootfs_path: "/rootfs".into(),
                    rootfs_sha256: "cd".repeat(32),
                    agent_protocol_version: 1,
                    boot_mode: kiln_runtime::image::BootMode::Init,
                },
                host: "fixture-host".into(),
                memory_mib: 256,
                vcpus: 1,
                source: None,
                operation: None,
                last_error: None,
                disk_copy: "Copy".into(),
                disk_layer: None,
                jail: None,
                network: None,
                last_launch: Some(kiln_runtime::launch_timing::LaunchTiming {
                    method: "create".into(),
                    cache: "uncontrolled".into(),
                    disk_copy: "Copy".into(),
                    storage_ms: Some(10.),
                    vmm_ms: Some(5.),
                    restore_ms: None,
                    agent_ms: None,
                    init_ms: None,
                    first_command_ms: None,
                    total_ms: Some(42.),
                }),
                owner: None,
            })
            .unwrap(),
        )
        .unwrap();
        let db = rusqlite::Connection::open(journal.join("operations.sqlite")).unwrap();
        db.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at,action)
             VALUES (?1,?2,?3,1,0,?4,?5,'create')",
            rusqlite::params![
                "0123456789abcdef0123456789abcde1",
                b"fingerprint-bytes",
                id,
                r#"{"id":"0123456789abcdef0123456789abcde1","box_id":"0123456789abcdef0123456789abcdef","state":"succeeded","result":{"type":"box","id":"0123456789abcdef0123456789abcdef","name":"measured","state":"running","memory_mib":256,"vcpus":1},"error":null}"#,
                1_700_000_000_i64,
            ],
        )
        .unwrap();
        let host = Host::open(&config).unwrap();
        let value = serde_json::to_value(host.dashboard().unwrap()).unwrap();
        assert_eq!(value["boxes"][0]["timing"]["total_ms"].as_f64(), Some(42.));
        crate::dashboard_check::parse_and_validate(&serde_json::to_vec(&value).unwrap())
            .expect("measured launch must satisfy the gate");
    }

    #[tokio::test]
    async fn ssh_key_lookup_does_not_consume_a_tunnel_permit() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let host = Host::open(&Config {
            runtime_dir: root.path().join("runtime"),
            journal_dir: root.path().join("journal"),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        })
        .unwrap();
        let operation = host
            .journal
            .lock()
            .unwrap()
            .accept(
                &Submit {
                    id: "c".repeat(32),
                    action: Action::Create {
                        template: "base".into(),
                        name: "box".into(),
                    },
                },
                &[],
                None,
            )
            .unwrap();
        let _held: Vec<_> = (0..32)
            .map(|_| host.sessions.clone().try_acquire_owned().unwrap())
            .collect();
        let response = ssh_key(State(host), Path(operation.box_id))
            .await
            .unwrap_err()
            .into_response();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(!body.contains("ssh_busy"), "{body}");
    }

    #[tokio::test]
    async fn create_wait_times_out_when_create_stays_running() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let host = Host::open(&Config {
            runtime_dir: root.path().join("runtime"),
            journal_dir: root.path().join("journal"),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        })
        .unwrap();
        let op = host
            .journal
            .lock()
            .unwrap()
            .accept(
                &Submit {
                    id: new_id(),
                    action: Action::Create {
                        template: "ubuntu".into(),
                        name: "slow".into(),
                    },
                },
                &[],
                None,
            )
            .unwrap();
        let err = wait_for_create(host, &op.id, 1).await.unwrap_err();
        assert_eq!(err.0, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(err.1.code, "wait_timeout");
        assert!(err.1.message.contains("still running"));
    }

    #[tokio::test]
    async fn create_wait_reports_control_plane_timeout_after_success() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let host = Host::open(&Config {
            runtime_dir: root.path().join("runtime"),
            journal_dir: root.path().join("journal"),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            jail_pool: 0,
            templates: Default::default(),
        })
        .unwrap();
        let op = host
            .journal
            .lock()
            .unwrap()
            .accept(
                &Submit {
                    id: new_id(),
                    action: Action::Create {
                        template: "ubuntu".into(),
                        name: "ghost".into(),
                    },
                },
                &[],
                None,
            )
            .unwrap();
        let mut finished = op.clone();
        finished.state = OperationState::Succeeded;
        finished.result = Some(Outcome::Box(BoxView {
            id: finished.box_id.clone(),
            name: "ghost".into(),
            state: "running".into(),
            memory_mib: 256,
            vcpus: 1,
            owner: None,
        }));
        finished.error = None;
        host.journal.lock().unwrap().finish(&finished).unwrap();
        let err = wait_for_create(host, &op.id, 1).await.unwrap_err();
        assert_eq!(err.0, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(err.1.code, "wait_timeout");
        assert_eq!(
            err.1.message,
            "The control plane has not observed the new box."
        );
    }
}
