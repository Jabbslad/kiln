//! Append-only sign-in journal. It sits beside the identity database and never
//! shares that database's prune path. Writers insert one row per attempt. The
//! request path does not wait on the journal.

use anyhow::Context;
use rusqlite::{Connection, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS signin_audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  account_id TEXT NOT NULL,
  tenant_id TEXT NOT NULL,
  client_ip TEXT NOT NULL,
  user_agent TEXT NOT NULL,
  client_id TEXT NOT NULL,
  session_id_hash TEXT NOT NULL,
  token_id_hash TEXT NOT NULL,
  auth_method TEXT NOT NULL,
  event TEXT NOT NULL,
  outcome TEXT NOT NULL,
  reason TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS signin_audit_account_ts ON signin_audit(account_id, ts);
CREATE TRIGGER IF NOT EXISTS signin_audit_no_update
BEFORE UPDATE ON signin_audit
BEGIN
  SELECT RAISE(ABORT, 'signin audit is append-only');
END;
CREATE TRIGGER IF NOT EXISTS signin_audit_no_delete
BEFORE DELETE ON signin_audit
BEGIN
  SELECT RAISE(ABORT, 'signin audit is append-only');
END;
"#;

pub(crate) const SIGN_IN: &str = "sign_in";
pub(crate) const OAUTH_CODE_EXCHANGE: &str = "oauth_code_exchange";
pub(crate) const DEVICE_APPROVAL: &str = "device_approval";
pub(crate) const TOKEN_ISSUE: &str = "token_issue";
pub(crate) const TOKEN_REFRESH: &str = "token_refresh";
pub(crate) const REVOCATION: &str = "revocation";
pub(crate) const APP_READER_EXCHANGE: &str = "app_reader_exchange";

pub(crate) struct Input {
    pub event: &'static str,
    pub outcome: &'static str,
    pub reason: &'static str,
    pub auth_method: &'static str,
    pub account_id: String,
    pub tenant_id: String,
    pub client_ip: String,
    pub user_agent: String,
    pub client_id: String,
    pub session_raw: String,
    pub token_raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SignInAuditRow {
    pub ts: String,
    pub account_id: String,
    pub tenant_id: String,
    pub client_ip: String,
    pub user_agent: String,
    pub client_id: String,
    pub session_id_hash: String,
    pub token_id_hash: String,
    pub auth_method: String,
    pub event: String,
    pub outcome: String,
    pub reason: String,
}

struct Row {
    ts: String,
    account_id: String,
    tenant_id: String,
    client_ip: String,
    user_agent: String,
    client_id: String,
    session_id_hash: String,
    token_id_hash: String,
    auth_method: String,
    event: String,
    outcome: String,
    reason: String,
}

enum Msg {
    Row(Box<Row>),
    Flush(mpsc::Sender<()>),
}

struct Inner {
    tx: Mutex<Option<SyncSender<Msg>>>,
    failures: Arc<AtomicU64>,
    thread: Mutex<Option<JoinHandle<()>>>,
    cleanup: Option<PathBuf>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.tx.lock().unwrap_or_else(|err| err.into_inner()).take();
        if let Some(handle) = self
            .thread
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
        {
            let _ = handle.join();
        }
        if let Some(dir) = self.cleanup.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

#[derive(Clone)]
pub(crate) struct AuditLog {
    inner: Arc<Inner>,
}

impl AuditLog {
    pub(crate) fn open(path: &Path) -> Self {
        start(path.to_path_buf(), None)
    }

    pub(crate) fn open_temp() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "kiln-signin-audit-{}",
            uuid::Uuid::new_v4().simple()
        ));
        if let Err(err) = fs::create_dir(&dir) {
            return disabled_with(err);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(err) = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)) {
                let _ = fs::remove_dir_all(&dir);
                return disabled_with(err);
            }
        }
        start(dir.join("signin-audit.db"), Some(dir))
    }

    pub(crate) fn record(&self, input: Input) {
        let row = Row {
            ts: utc_millis(now_ms()),
            account_id: bound(&input.account_id, 256),
            tenant_id: bound(&input.tenant_id, 256),
            client_ip: bound(&input.client_ip, 256),
            user_agent: bound(&input.user_agent, 200),
            client_id: bound(&input.client_id, 256),
            session_id_hash: hex_hash(&input.session_raw),
            token_id_hash: hex_hash(&input.token_raw),
            auth_method: bound(input.auth_method, 256),
            event: bound(input.event, 256),
            outcome: bound(input.outcome, 256),
            reason: bound(input.reason, 256),
        };
        let sender = self
            .inner
            .tx
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        match sender {
            Some(tx) => {
                if tx.try_send(Msg::Row(Box::new(row))).is_err() {
                    self.fail();
                }
            }
            None => self.fail(),
        }
    }

    pub(crate) fn flush(&self) {
        let Some(tx) = self
            .inner
            .tx
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
        else {
            return;
        };
        let (ack_tx, ack_rx) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match tx.try_send(Msg::Flush(ack_tx.clone())) {
                Ok(()) => break,
                Err(TrySendError::Full(_)) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
        let _ = ack_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    }

    pub(crate) fn failures(&self) -> u64 {
        self.inner.failures.load(Ordering::Relaxed)
    }

    fn fail(&self) {
        self.inner.failures.fetch_add(1, Ordering::Relaxed);
        eprintln!("signin audit store is unavailable");
    }
}

fn disabled_with(err: std::io::Error) -> AuditLog {
    eprintln!("signin audit store is unavailable");
    let _ = err;
    AuditLog {
        inner: Arc::new(Inner {
            tx: Mutex::new(None),
            failures: Arc::new(AtomicU64::new(1)),
            thread: Mutex::new(None),
            cleanup: None,
        }),
    }
}

fn start(path: PathBuf, cleanup: Option<PathBuf>) -> AuditLog {
    let (tx, rx) = mpsc::sync_channel(256);
    let failures = Arc::new(AtomicU64::new(0));
    let counted = failures.clone();
    let thread = thread::Builder::new()
        .name("kiln-signin-audit".into())
        .spawn(move || worker(path, rx, counted))
        .ok();
    if thread.is_none() {
        failures.fetch_add(1, Ordering::Relaxed);
        eprintln!("signin audit store is unavailable");
    }
    AuditLog {
        inner: Arc::new(Inner {
            tx: Mutex::new(thread.is_some().then_some(tx)),
            failures,
            thread: Mutex::new(thread),
            cleanup,
        }),
    }
}

fn worker(path: PathBuf, rx: Receiver<Msg>, failures: Arc<AtomicU64>) {
    let conn = match open_db(&path) {
        Ok(conn) => Some(conn),
        Err(_) => {
            failures.fetch_add(1, Ordering::Relaxed);
            eprintln!("signin audit store is unavailable");
            None
        }
    };
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Row(row) => match conn.as_ref() {
                Some(conn) => {
                    if insert(conn, &row).is_err() {
                        failures.fetch_add(1, Ordering::Relaxed);
                        eprintln!("signin audit store is unavailable");
                    }
                }
                None => {
                    failures.fetch_add(1, Ordering::Relaxed);
                    eprintln!("signin audit store is unavailable");
                }
            },
            Msg::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

fn open_db(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("signin audit path must be a regular file");
        }
        Ok(meta) if !meta.is_file() => anyhow::bail!("signin audit path must be a regular file"),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(path)?;
        }
        Err(err) => return Err(err.into()),
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn insert(conn: &Connection, row: &Row) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO signin_audit(
            ts, account_id, tenant_id, client_ip, user_agent, client_id,
            session_id_hash, token_id_hash, auth_method, event, outcome, reason
        ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            row.ts,
            row.account_id,
            row.tenant_id,
            row.client_ip,
            row.user_agent,
            row.client_id,
            row.session_id_hash,
            row.token_id_hash,
            row.auth_method,
            row.event,
            row.outcome,
            row.reason,
        ],
    )?;
    Ok(())
}

pub fn signin_audit_path(database: &Path) -> PathBuf {
    let name = database
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "identity.db".into());
    database.with_file_name(format!("{name}.signin-audit"))
}

pub fn query_signin_audit(
    database: &Path,
    account: &str,
    since: &str,
    until: &str,
) -> anyhow::Result<Vec<SignInAuditRow>> {
    anyhow::ensure!(
        !account.is_empty() && account.len() <= 256,
        "invalid account"
    );
    anyhow::ensure!(valid_ts(since) && valid_ts(until), "invalid timestamp");
    read_filtered(database, Some((account, since, until)))
}

pub fn read_signin_audit(database: &Path) -> anyhow::Result<Vec<SignInAuditRow>> {
    read_filtered(database, None)
}

fn read_filtered(
    database: &Path,
    filter: Option<(&str, &str, &str)>,
) -> anyhow::Result<Vec<SignInAuditRow>> {
    let path = signin_audit_path(database);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| "signin audit store is unavailable")?;
    conn.busy_timeout(Duration::from_secs(2))?;
    let (sql, account, since, until) = match filter {
        Some((account, since, until)) => (
            "SELECT ts, account_id, tenant_id, client_ip, user_agent, client_id,
                    session_id_hash, token_id_hash, auth_method, event, outcome, reason
             FROM signin_audit
             WHERE account_id=?1 AND ts>=?2 AND ts<=?3
             ORDER BY id",
            account,
            since,
            until,
        ),
        None => (
            "SELECT ts, account_id, tenant_id, client_ip, user_agent, client_id,
                    session_id_hash, token_id_hash, auth_method, event, outcome, reason
             FROM signin_audit ORDER BY id",
            "",
            "",
            "",
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = if filter.is_some() {
        stmt.query_map(params![account, since, until], row_from)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], row_from)?
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

fn row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<SignInAuditRow> {
    Ok(SignInAuditRow {
        ts: row.get(0)?,
        account_id: row.get(1)?,
        tenant_id: row.get(2)?,
        client_ip: row.get(3)?,
        user_agent: row.get(4)?,
        client_id: row.get(5)?,
        session_id_hash: row.get(6)?,
        token_id_hash: row.get(7)?,
        auth_method: row.get(8)?,
        event: row.get(9)?,
        outcome: row.get(10)?,
        reason: row.get(11)?,
    })
}

fn valid_ts(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 24
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) || byte.is_ascii_digit()
        })
}

fn bound(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn hex_hash(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(raw.as_bytes()) {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| i64::try_from(dur.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(crate) fn utc_millis(unix_ms: i64) -> String {
    let secs = unix_ms.div_euclid(1000);
    let ms = unix_ms.rem_euclid(1000) as u32;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::utc_millis;

    #[test]
    fn utc_epoch_and_known_instant_are_zero_padded() {
        assert_eq!(utc_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(utc_millis(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
    }
}
