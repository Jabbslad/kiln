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
    Action, ApiError, BoxView, Operation, OperationState, Outcome, Submit, TemplateView,
    valid_alias, valid_id,
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
        runtime_lock.try_lock()?;
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
        };
        Ok(Outcome::Box(view(record)))
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

async fn templates(State(host): State<Arc<Host>>) -> Json<Vec<TemplateView>> {
    Json(host.catalog.clone())
}

async fn ssh_key(
    State(host): State<Arc<Host>>,
    Path(id): Path<String>,
) -> Result<Json<kiln_api::SshReady>, Failure> {
    host.owns(&id)?;
    let _permit = host
        .sessions
        .try_acquire()
        .map_err(|_| crate::ssh::busy())?;
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
