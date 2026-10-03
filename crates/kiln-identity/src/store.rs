use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kiln_api::auth::{DeviceAuthorization, ServerDescriptor};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Store {
    pub(crate) db: Arc<Mutex<Connection>>,
}
pub(crate) fn hash(s: &str) -> Vec<u8> {
    Sha256::digest(s.as_bytes()).to_vec()
}
pub(crate) fn secret() -> String {
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
pub(crate) fn code() -> String {
    const A: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut b = [0u8; 12];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| A[*x as usize % A.len()] as char).collect()
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            let metadata = std::fs::symlink_metadata(parent)?;
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "database directory must not be a symlink"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                anyhow::ensure!(
                    metadata.mode() & 0o077 == 0 && metadata.uid() == unsafe { libc::geteuid() },
                    "database directory must be owned by this user and owner-only"
                );
            }
        }
        // Create privately before SQLite opens the file; its WAL inherits this mode.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        for candidate in [
            path.to_path_buf(),
            path.with_file_name(format!(
                "{}-wal",
                path.file_name().unwrap().to_string_lossy()
            )),
            path.with_file_name(format!(
                "{}-shm",
                path.file_name().unwrap().to_string_lossy()
            )),
        ] {
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => crate::check_private_file(&candidate)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let c = Connection::open(path)?;
        let version: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
        anyhow::ensure!(
            version == 0 || version == 1,
            "unsupported identity schema; restore a compatible backup"
        );
        if version == 0 {
            let tables: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |r| r.get(0))?;
            anyhow::ensure!(
                tables == 0,
                "unversioned identity database; automatic migration is not supported"
            );
        }
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "FULL")?;
        c.execute_batch(SCHEMA)?;
        c.pragma_update(None, "user_version", 1)?;
        Ok(Self {
            db: Arc::new(Mutex::new(c)),
        })
    }
    pub fn open_memory() -> anyhow::Result<Self> {
        let c = Connection::open_in_memory()?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.execute_batch(SCHEMA)?;
        Ok(Self {
            db: Arc::new(Mutex::new(c)),
        })
    }
    pub fn create_device(
        &self,
        name: &str,
        ip: &str,
        now: i64,
    ) -> anyhow::Result<DeviceAuthorization> {
        anyhow::ensure!(!name.is_empty() && name.len() <= 128, "invalid device name");
        let mut c = self.db.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        rate(&tx, &format!("device:{ip}"), now, 10, 60)?;
        let n: i64 = tx.query_row(
            "SELECT (SELECT count(*) FROM pending_devices WHERE expires_at>?1) + (SELECT count(*) FROM pending_registrations WHERE expires_at>?1)",
            [now],
            |r| r.get(0),
        )?;
        anyhow::ensure!(n < 10000, "too many pending requests");
        let raw = secret();
        let user = code();
        tx.execute("INSERT INTO pending_devices(secret_hash,user_code,name,state,expires_at,next_poll,interval) VALUES(?1,?2,?3,'pending',?4,?5,5)",params![hash(&raw),user,name,now+600,now+5])?;
        tx.commit()?;
        Ok(DeviceAuthorization {
            device_code: raw,
            user_code: user.clone(),
            verification_uri: String::new(),
            verification_uri_complete: String::new(),
            expires_in: 600,
            interval: 5,
        })
    }
    pub fn seed_account(
        &self,
        provider: &str,
        subject: &str,
        display: &str,
        now: i64,
    ) -> anyhow::Result<String> {
        let mut c = self.db.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(v) = tx
            .query_row(
                "SELECT account_id FROM provider_identities WHERE provider=?1 AND subject=?2",
                params![provider, subject],
                |r| r.get(0),
            )
            .optional()?
        {
            tx.commit()?;
            return Ok(v);
        }
        let a = id("acct");
        tx.execute(
            "INSERT INTO accounts(id,created_at)VALUES(?1,?2)",
            params![a, now],
        )?;
        tx.execute("INSERT INTO provider_identities(provider,subject,account_id,display_name)VALUES(?1,?2,?3,?4)",params![provider,subject,a,display])?;
        tx.commit()?;
        Ok(a)
    }
    pub fn approve_code(
        &self,
        user: &str,
        account: &str,
        approve: bool,
        now: i64,
    ) -> anyhow::Result<()> {
        let mut c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("database unavailable"))?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = if approve { "approved" } else { "denied" };
        let n=tx.execute("UPDATE pending_devices SET state=?1,account_id=?2 WHERE user_code=?3 AND state='pending' AND expires_at>?4 AND EXISTS(SELECT 1 FROM accounts WHERE id=?2 AND revoked_at IS NULL)",params![state,account,user,now])?;
        anyhow::ensure!(n == 1, "invalid or consumed code");
        tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('device_decision',?1,?2,?3,?4)", params![account,user,state,now])?;
        tx.commit()?;
        Ok(())
    }
    pub fn create_server(
        &self,
        owner: &str,
        name: &str,
        origin: &str,
        ca: &str,
        now: i64,
    ) -> anyhow::Result<(String, String)> {
        let sid = id("srv");
        let token = secret();
        let c = self.db.lock().unwrap();
        c.execute("INSERT INTO servers(id,owner_id,name,origin,ca_pem,revision,active,directory_hash,created_at)VALUES(?1,?2,?3,?4,?5,1,1,?6,?7)",params![sid,owner,name,origin,ca,hash(&token),now])?;
        Ok((sid, token))
    }
    pub fn servers(&self, owner: &str) -> anyhow::Result<Vec<ServerDescriptor>> {
        let c = self.db.lock().unwrap();
        let mut q = c.prepare(
            "SELECT id,name,origin,ca_pem,revision FROM servers WHERE owner_id=?1 AND active=1",
        )?;
        Ok(q.query_map([owner], |r| {
            Ok(ServerDescriptor {
                id: r.get(0)?,
                name: r.get(1)?,
                origin: r.get(2)?,
                ca_pem: r.get(3)?,
                revision: r.get::<_, i64>(4)? as u64,
            })
        })?
        .collect::<Result<_, _>>()?)
    }
    pub fn prune(&self, now: i64) -> anyhow::Result<()> {
        let c = self.db.lock().unwrap();
        c.execute(
            "DELETE FROM refresh_tokens WHERE absolute_expiry<=?1",
            [now],
        )?;
        c.execute(
            "UPDATE servers SET directory_hash=randomblob(32) WHERE active=0 AND created_at<=?1",
            [now - 3600],
        )?;
        c.execute(
            "DELETE FROM pending_devices WHERE expires_at<?1",
            [now - 3600],
        )?;
        c.execute("DELETE FROM central_tokens WHERE expires_at<?1", [now])?;
        c.execute("DELETE FROM browser_sessions WHERE expires_at<?1", [now])?;
        c.execute("DELETE FROM oauth_attempts WHERE expires_at<?1", [now])?;
        c.execute(
            "DELETE FROM pending_registrations WHERE expires_at<?1",
            [now - 3600],
        )?;
        c.execute(
            "DELETE FROM rate_limits WHERE window_start<?1",
            [now - 3600],
        )?;
        c.execute(
            "DELETE FROM audit_events WHERE created_at<?1",
            [now - 90 * 86400],
        )?;
        Ok(())
    }
}
pub(crate) fn rate(
    c: &Connection,
    key: &str,
    now: i64,
    max: i64,
    window: i64,
) -> anyhow::Result<()> {
    c.execute(
        "DELETE FROM rate_limits WHERE key=?1 AND window_start<=?2",
        params![key, now - window],
    )?;
    let count: i64 = c
        .query_row("SELECT count FROM rate_limits WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or(0);
    anyhow::ensure!(count < max, "rate limit exceeded");
    c.execute("INSERT INTO rate_limits(key,window_start,count)VALUES(?1,?2,1) ON CONFLICT(key) DO UPDATE SET count=count+1",params![key,now])?;
    Ok(())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS provider_identities(provider TEXT NOT NULL,subject TEXT NOT NULL,account_id TEXT NOT NULL REFERENCES accounts(id),display_name TEXT NOT NULL,PRIMARY KEY(provider,subject));
CREATE TABLE IF NOT EXISTS browser_sessions(hash BLOB PRIMARY KEY,csrf_hash BLOB NOT NULL,requested_code TEXT,account_id TEXT REFERENCES accounts(id),expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS oauth_attempts(state_hash BLOB PRIMARY KEY,session_hash BLOB NOT NULL,provider TEXT NOT NULL,pkce TEXT NOT NULL,nonce TEXT,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pending_devices(secret_hash BLOB PRIMARY KEY,user_code TEXT UNIQUE NOT NULL,name TEXT NOT NULL,state TEXT NOT NULL,expires_at INTEGER NOT NULL,next_poll INTEGER NOT NULL,interval INTEGER NOT NULL,account_id TEXT REFERENCES accounts(id));
CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY,account_id TEXT NOT NULL REFERENCES accounts(id),name TEXT NOT NULL,created_at INTEGER NOT NULL,last_used INTEGER NOT NULL,revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS refresh_tokens(hash BLOB PRIMARY KEY,family_id TEXT NOT NULL,device_id TEXT NOT NULL REFERENCES devices(id),created_at INTEGER NOT NULL,last_used INTEGER NOT NULL,absolute_expiry INTEGER NOT NULL,spent_at INTEGER,family_revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS central_tokens(hash BLOB PRIMARY KEY,device_id TEXT NOT NULL REFERENCES devices(id),family_id TEXT,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pending_registrations(secret_hash BLOB PRIMARY KEY,user_code TEXT UNIQUE NOT NULL,name TEXT NOT NULL,origin TEXT NOT NULL,ca_pem TEXT NOT NULL,state TEXT NOT NULL,owner_id TEXT REFERENCES accounts(id),expires_at INTEGER NOT NULL,poll_consumed INTEGER NOT NULL DEFAULT 0,next_poll INTEGER NOT NULL,interval INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS servers(id TEXT PRIMARY KEY,owner_id TEXT NOT NULL REFERENCES accounts(id),name TEXT NOT NULL,origin TEXT NOT NULL,ca_pem TEXT NOT NULL,revision INTEGER NOT NULL,active INTEGER NOT NULL,directory_hash BLOB NOT NULL,created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS automation_keys(id TEXT PRIMARY KEY,owner_id TEXT NOT NULL REFERENCES accounts(id),server_id TEXT NOT NULL REFERENCES servers(id),hash BLOB UNIQUE NOT NULL,scope TEXT NOT NULL,expires_at INTEGER NOT NULL,revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS rate_limits(key TEXT PRIMARY KEY,window_start INTEGER NOT NULL,count INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS audit_events(id INTEGER PRIMARY KEY,event TEXT NOT NULL,actor_id TEXT,resource_id TEXT,outcome TEXT NOT NULL,created_at INTEGER NOT NULL);
"#;
