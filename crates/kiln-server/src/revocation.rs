//! Live revocation set polled from identity and enforced on every JWT.
use anyhow::{Context, Result, ensure};
use kiln_api::auth::{RevocationQuery, RevocationSet, Role};
use std::sync::Mutex;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;

const REFRESH: Duration = Duration::from_secs(2);
const MEMBER_STALE: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct StreamCancel {
    revoked: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl StreamCancel {
    pub fn new() -> Self {
        Self {
            revoked: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.revoked.load(Ordering::Relaxed)
    }

    pub async fn wait(&self) {
        if self.is_cancelled() {
            return;
        }
        self.notify.notified().await;
    }

    fn trigger(&self) {
        if self
            .revoked
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.notify.notify_waiters();
        }
    }
}

struct LiveSession {
    sub: String,
    credential_id: String,
    cancel: StreamCancel,
}

#[derive(Default)]
struct Snapshot {
    accounts: HashSet<String>,
    credentials: HashSet<String>,
    refreshed: Option<Instant>,
}

pub struct RevocationWatch {
    enrollment: crate::auth::Enrollment,
    login_secret: String,
    http: reqwest::Client,
    snapshot: Mutex<Snapshot>,
    sessions: Mutex<Vec<LiveSession>>,
}

impl RevocationWatch {
    pub fn new(
        enrollment: crate::auth::Enrollment,
        login_secret: String,
        http: reqwest::Client,
    ) -> Arc<Self> {
        let watch = Arc::new(Self {
            enrollment,
            login_secret,
            http,
            snapshot: Mutex::new(Snapshot::default()),
            sessions: Mutex::new(Vec::new()),
        });
        let task = Arc::clone(&watch);
        tokio::spawn(async move {
            let _ = task.refresh().await;
            let mut interval = tokio::time::interval(REFRESH);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let _ = task.refresh().await;
            }
        });
        watch
    }

    pub fn track(&self, sub: &str, credential_id: &str) -> StreamCancel {
        let cancel = StreamCancel::new();
        self.sessions.lock().unwrap().push(LiveSession {
            sub: sub.into(),
            credential_id: credential_id.into(),
            cancel: cancel.clone(),
        });
        cancel
    }

    pub fn allow(&self, sub: &str, credential_id: &str, role: Role) -> Result<()> {
        let snap = self.snapshot.lock().unwrap();
        if snap.accounts.contains(sub) || snap.credentials.contains(credential_id) {
            anyhow::bail!("revoked");
        }
        if role == Role::Member && snap.refreshed.is_none_or(|t| t.elapsed() > MEMBER_STALE) {
            anyhow::bail!("revocation stale");
        }
        Ok(())
    }

    async fn refresh(&self) -> Result<()> {
        let url = format!(
            "{}/v1/revocations",
            self.enrollment.issuer.trim_end_matches('/')
        );
        let mut response = self
            .http
            .post(url)
            .bearer_auth(&self.login_secret)
            .json(&RevocationQuery {
                tenant_id: self.enrollment.owner_id.clone(),
            })
            .send()
            .await
            .context("revocation fetch failed")?;
        ensure!(response.status().is_success(), "revocation fetch rejected");
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(bytes.len() + chunk.len() <= 65536, "revocation too large");
            bytes.extend_from_slice(&chunk);
        }
        let set: RevocationSet = serde_json::from_slice(&bytes)?;
        for id in &set.accounts {
            ensure!(!id.is_empty() && id.len() <= 128, "invalid account id");
        }
        for id in &set.credentials {
            ensure!(!id.is_empty() && id.len() <= 128, "invalid credential id");
        }
        let accounts: HashSet<_> = set.accounts.into_iter().collect();
        let credentials: HashSet<_> = set.credentials.into_iter().collect();
        {
            let mut snap = self.snapshot.lock().unwrap();
            snap.accounts = accounts.clone();
            snap.credentials = credentials.clone();
            snap.refreshed = Some(Instant::now());
        }
        let mut sessions = self.sessions.lock().unwrap();
        for session in sessions.iter() {
            if accounts.contains(&session.sub) || credentials.contains(&session.credential_id) {
                session.cancel.trigger();
            }
        }
        sessions.retain(|s| !s.cancel.is_cancelled());
        Ok(())
    }
}

pub fn read_login_secret(path: &std::path::Path) -> Result<Option<String>> {
    use std::{fs, os::unix::fs::MetadataExt};
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file() && meta.uid() == 0 && meta.mode() & 0o027 == 0 && meta.len() <= 8192,
        "login secret must be a root-owned private regular file"
    );
    let token = String::from_utf8(fs::read(path)?)?;
    ensure!(
        !token.is_empty() && token.len() <= 8192 && !token.bytes().any(|b| b.is_ascii_control()),
        "invalid login secret"
    );
    Ok(Some(token))
}
