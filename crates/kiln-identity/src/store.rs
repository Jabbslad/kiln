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
            version == 0 || version == 1 || version == 2 || version == 3,
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
        let mut version = version;
        if version == 1 {
            let tx = c.unchecked_transaction()?;
            add_column(&tx, "redirect_uri", "TEXT")?;
            add_column(&tx, "login_secret_hash", "BLOB")?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
            version = 2;
        } else if version == 2
            && (!column_present(&c, "redirect_uri")? || !column_present(&c, "login_secret_hash")?)
        {
            anyhow::bail!("unsupported identity schema; restore a compatible backup");
        }
        if version == 2 {
            let tx = c.unchecked_transaction()?;
            migrate_tenants_v3(&tx)?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
            version = 3;
        } else if version == 3
            && (!table_present(&c, "tenants")? || !column_present(&c, "tenant_id")?)
        {
            anyhow::bail!("unsupported identity schema; restore a compatible backup");
        }
        if version == 0 || version == 3 {
            c.pragma_update(None, "user_version", 3)?;
        }
        Ok(Self {
            db: Arc::new(Mutex::new(c)),
        })
    }
    pub fn open_memory() -> anyhow::Result<Self> {
        let c = Connection::open_in_memory()?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.execute_batch(SCHEMA)?;
        c.pragma_update(None, "user_version", 3)?;
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
        let mut c = self.db.lock().unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let tenant = ensure_owner_tenant(&tx, owner, now)?;
        tx.execute(
            "INSERT INTO servers(id,owner_id,tenant_id,name,origin,ca_pem,revision,active,directory_hash,created_at)VALUES(?1,?2,?3,?4,?5,?6,1,1,?7,?8)",
            params![sid, owner, tenant, name, origin, ca, hash(&token), now],
        )?;
        set_default_server(&tx, &tenant, &sid)?;
        tx.commit()?;
        Ok((sid, token))
    }
    pub fn servers(&self, owner: &str) -> anyhow::Result<Vec<ServerDescriptor>> {
        let c = self.db.lock().unwrap();
        let mut q = c.prepare(
            "SELECT s.id,s.name,s.origin,s.ca_pem,s.revision FROM servers s
             JOIN tenant_members m ON m.tenant_id=s.tenant_id
             WHERE m.account_id=?1 AND s.active=1 ORDER BY s.created_at",
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
        c.execute("DELETE FROM server_codes WHERE expires_at<?1", [now])?;
        c.execute(
            "DELETE FROM server_authorizations WHERE expires_at<?1",
            [now],
        )?;
        c.execute("DELETE FROM app_readers WHERE expires_at<?1", [now])?;
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
fn table_present(c: &Connection, table: &str) -> anyhow::Result<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get(0),
    )?;
    Ok(n == 1)
}
fn migrate_tenants_v3(c: &Connection) -> anyhow::Result<()> {
    c.execute_batch(
        "DROP TABLE IF EXISTS server_members;
         CREATE TABLE IF NOT EXISTS tenants(
            id TEXT PRIMARY KEY,
            owner_account_id TEXT NOT NULL UNIQUE REFERENCES accounts(id),
            default_server_id TEXT,
            created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tenant_members(
            tenant_id TEXT NOT NULL REFERENCES tenants(id),
            account_id TEXT NOT NULL REFERENCES accounts(id),
            role TEXT NOT NULL CHECK(role IN ('owner','member')),
            local_name TEXT,
            PRIMARY KEY(tenant_id,account_id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS tenant_members_local_name
            ON tenant_members(tenant_id,local_name)
            WHERE local_name IS NOT NULL;",
    )?;
    add_column(c, "tenant_id", "TEXT REFERENCES tenants(id)")?;
    let mut owners = c.prepare("SELECT DISTINCT owner_id FROM servers ORDER BY owner_id")?;
    let owners = owners
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for owner in owners {
        let tenant = id("ten");
        let created: i64 = c.query_row(
            "SELECT min(created_at) FROM servers WHERE owner_id=?1",
            [&owner],
            |r| r.get(0),
        )?;
        c.execute(
            "INSERT INTO tenants(id,owner_account_id,created_at)VALUES(?1,?2,?3)",
            params![tenant, owner, created],
        )?;
        c.execute(
            "INSERT INTO tenant_members(tenant_id,account_id,role)VALUES(?1,?2,'owner')",
            params![tenant, owner],
        )?;
        c.execute(
            "UPDATE servers SET tenant_id=?1 WHERE owner_id=?2",
            params![tenant, owner],
        )?;
        let default: Option<String> = c
            .query_row(
                "SELECT id FROM servers WHERE owner_id=?1 ORDER BY created_at LIMIT 1",
                [&owner],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(server) = default {
            c.execute(
                "UPDATE tenants SET default_server_id=?1 WHERE id=?2",
                params![server, tenant],
            )?;
        }
    }
    Ok(())
}
pub(crate) fn ensure_owner_tenant(c: &Connection, owner: &str, now: i64) -> anyhow::Result<String> {
    if let Some(tenant) = c
        .query_row(
            "SELECT id FROM tenants WHERE owner_account_id=?1",
            [owner],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(tenant);
    }
    let tenant = id("ten");
    c.execute(
        "INSERT INTO tenants(id,owner_account_id,created_at)VALUES(?1,?2,?3)",
        params![tenant, owner, now],
    )?;
    c.execute(
        "INSERT INTO tenant_members(tenant_id,account_id,role)VALUES(?1,?2,'owner')",
        params![tenant, owner],
    )?;
    Ok(tenant)
}
pub(crate) fn set_default_server(
    c: &Connection,
    tenant: &str,
    server_id: &str,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE tenants SET default_server_id=?1 WHERE id=?2 AND default_server_id IS NULL",
        params![server_id, tenant],
    )?;
    Ok(())
}
fn column_present(c: &Connection, column: &str) -> anyhow::Result<bool> {
    let mut stmt = c.prepare("PRAGMA table_info(servers)")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
fn add_column(c: &Connection, column: &str, kind: &str) -> anyhow::Result<()> {
    if !column_present(c, column)? {
        c.execute(
            &format!("ALTER TABLE servers ADD COLUMN {column} {kind}"),
            [],
        )?;
    }
    Ok(())
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
CREATE TABLE IF NOT EXISTS tenants(id TEXT PRIMARY KEY,owner_account_id TEXT NOT NULL UNIQUE REFERENCES accounts(id),default_server_id TEXT,created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS tenant_members(tenant_id TEXT NOT NULL REFERENCES tenants(id),account_id TEXT NOT NULL REFERENCES accounts(id),role TEXT NOT NULL CHECK(role IN ('owner','member')),local_name TEXT,PRIMARY KEY(tenant_id,account_id));
CREATE UNIQUE INDEX IF NOT EXISTS tenant_members_local_name ON tenant_members(tenant_id,local_name) WHERE local_name IS NOT NULL;
CREATE TABLE IF NOT EXISTS servers(id TEXT PRIMARY KEY,owner_id TEXT NOT NULL REFERENCES accounts(id),tenant_id TEXT REFERENCES tenants(id),name TEXT NOT NULL,origin TEXT NOT NULL,ca_pem TEXT NOT NULL,revision INTEGER NOT NULL,active INTEGER NOT NULL,directory_hash BLOB NOT NULL,created_at INTEGER NOT NULL,redirect_uri TEXT,login_secret_hash BLOB);
CREATE TABLE IF NOT EXISTS server_codes(hash BLOB PRIMARY KEY,server_id TEXT NOT NULL,account_id TEXT NOT NULL,redirect_uri TEXT NOT NULL,challenge TEXT NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS server_authorizations(session_hash BLOB PRIMARY KEY,server_id TEXT NOT NULL,redirect_uri TEXT NOT NULL,state TEXT NOT NULL,challenge TEXT NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS app_readers(hash BLOB PRIMARY KEY,server_id TEXT NOT NULL,account_id TEXT NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS automation_keys(id TEXT PRIMARY KEY,owner_id TEXT NOT NULL REFERENCES accounts(id),server_id TEXT NOT NULL REFERENCES servers(id),hash BLOB UNIQUE NOT NULL,scope TEXT NOT NULL,expires_at INTEGER NOT NULL,revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS rate_limits(key TEXT PRIMARY KEY,window_start INTEGER NOT NULL,count INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS audit_events(id INTEGER PRIMARY KEY,event TEXT NOT NULL,actor_id TEXT,resource_id TEXT,outcome TEXT NOT NULL,created_at INTEGER NOT NULL);
"#;
