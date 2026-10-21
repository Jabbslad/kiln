use crate::{
    Failure,
    journal::{Conflict, Journal},
};
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
};
use kiln_api::{
    Action, ApiError, BoxView, CheckpointView, Operation, OperationState, Outcome, Submit,
    TemplateView, valid_alias, valid_id,
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
    pub templates: BTreeMap<String, String>,
}

pub struct Host {
    runtime: Runtime,
    runtime_dir: PathBuf,
    templates: BTreeMap<String, String>,
    catalog: Vec<TemplateView>,
    journal: Mutex<Journal>,
    capacity: Arc<Semaphore>,
    sessions: Arc<Semaphore>,
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
        Ok(Arc::new(Self {
            runtime,
            runtime_dir: config.runtime_dir.clone(),
            templates: config.templates.clone(),
            catalog,
            journal: Mutex::new(journal),
            capacity: Arc::new(Semaphore::new(8)),
            sessions: Arc::new(Semaphore::new(32)),
            _locks: [runtime_lock, journal_lock],
        }))
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
        drop(journal);
        let mut names = BTreeMap::new();
        let mut boxes = Vec::new();
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
            let disk_bytes = box_disk(&self.runtime_dir, &record).and_then(file_len);
            names.insert(id.clone(), record.name.clone());
            boxes.push(crate::dashboard::BoxFacts {
                id,
                name: record.name,
                state: record.state,
                memory_mib: record.memory_mib,
                vcpus: record.vcpus,
                disk_bytes,
                created_at,
                last_operation: record.operation,
                last_error: record.last_error,
                restored_from: record.source,
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
        Ok(crate::dashboard::document(
            "",
            &crate::dashboard::utc_timestamp(crate::dashboard::unix_now()),
            "online",
            &crate::dashboard::Inventory {
                boxes,
                templates,
                operations,
                checkpoints,
            },
        ))
    }
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
    Ok(crate::ssh::upgrade(request, stream, permit))
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

async fn submit(
    State(host): State<Arc<Host>>,
    Json(request): Json<Submit>,
) -> Result<(StatusCode, Json<Operation>), Failure> {
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
    let mut db = host.journal.lock().unwrap();
    if let Some(existing) = db.existing(&request).map_err(|e| {
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
        return Ok((StatusCode::OK, Json(existing)));
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
    let accepted = db.accept(&request).map_err(Failure::internal)?;
    let mut op = accepted.clone();
    drop(db);
    let worker_host = host.clone();
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
                op.error = Some(ApiError { code: "runtime_operation_failed".into(), message: "Operation did not return a confirmed result. Inspect the box before issuing another command; consult the host log for lifecycle failures.".into() });
                // Exec inputs and output are never logged, including on error.
                if !is_exec {
                    eprintln!("operation {}: {other:?}", op.id);
                }
            }
        }
        if let Err(error) = host.journal.lock().unwrap().finish(&op) {
            eprintln!("could not persist operation {} outcome: {error}", op.id);
        }
    });
    Ok((StatusCode::ACCEPTED, Json(accepted)))
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
            templates: Default::default(),
        })
        .unwrap();
        let operation = host
            .journal
            .lock()
            .unwrap()
            .accept(&Submit {
                id: "c".repeat(32),
                action: Action::Create {
                    template: "base".into(),
                    name: "box".into(),
                },
            })
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
}
