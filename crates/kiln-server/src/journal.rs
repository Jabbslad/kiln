use anyhow::{Result, ensure};
use kiln_api::{Action, ApiError, Operation, OperationState, Submit, new_id};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

/// Rows at or above this count trigger retention before accepting another operation.
pub const HIGH_WATER_MARK: u32 = 10_000;
/// Terminal rows older than this are eligible for pruning.
pub const RETENTION_SECS: i64 = 30 * 86400;
/// Newest terminal rows kept even when older than [`RETENTION_SECS`].
pub const TERMINAL_KEEP_COUNT: i64 = 8_000;

#[derive(Debug)]
pub struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request ID already used for a different action")
    }
}
impl std::error::Error for Conflict {}

#[derive(Debug)]
pub struct JournalFull;
impl std::fmt::Display for JournalFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "operation journal is full after retention; archive or delete old boxes before accepting more work",
        )
    }
}
impl std::error::Error for JournalFull {}

pub struct Journal(Connection);

const SCHEMA_VERSION: u32 = operations_journal_max_version();

const fn operations_journal_max_version() -> u32 {
    const RAW: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/operations-journal.max-version"
    ));
    let bytes = RAW.as_bytes();
    let mut value = 0u32;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' || byte == b'\r' {
            break;
        }
        if byte.is_ascii_digit() {
            value = value * 10 + (byte - b'0') as u32;
        } else if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        } else {
            panic!("operations-journal.max-version must contain a decimal version");
        }
        index += 1;
    }
    assert!(value > 0, "operations-journal.max-version must be positive");
    value
}

fn table_has_column(connection: &Connection, column: &str) -> Result<bool> {
    let mut statement = connection.prepare("PRAGMA table_info(operations)")?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(names.filter_map(Result::ok).any(|name| name == column))
}

const TIMESTAMP_MS_THRESHOLD: i64 = 1_000_000_000_000;

fn migrate_seconds_timestamps_to_ms(connection: &Connection) -> Result<()> {
    connection.execute(
        "UPDATE operations SET created_at = created_at * 1000
         WHERE created_at > 0 AND created_at < ?1",
        params![TIMESTAMP_MS_THRESHOLD],
    )?;
    connection.execute(
        "UPDATE operations SET finished_at = finished_at * 1000
         WHERE finished_at IS NOT NULL AND finished_at > 0 AND finished_at < ?1",
        params![TIMESTAMP_MS_THRESHOLD],
    )?;
    Ok(())
}

pub(crate) fn journal_stored_secs(stored: i64) -> i64 {
    if stored >= TIMESTAMP_MS_THRESHOLD {
        stored / 1000
    } else {
        stored
    }
}

fn ensure_v2_columns(connection: &Connection) -> Result<()> {
    if !table_has_column(connection, "finished_at")? {
        connection.execute("ALTER TABLE operations ADD COLUMN finished_at INTEGER", [])?;
    }
    if !table_has_column(connection, "action")? {
        connection.execute(
            "ALTER TABLE operations ADD COLUMN action TEXT NOT NULL DEFAULT 'unknown'",
            [],
        )?;
    }
    Ok(())
}

fn ensure_v4_columns(connection: &Connection) -> Result<()> {
    if !table_has_column(connection, "actor")? {
        // Nullable: legacy rows and pre-gateway accepts have no actor.
        connection.execute("ALTER TABLE operations ADD COLUMN actor TEXT", [])?;
    }
    Ok(())
}

impl Journal {
    // The host holds a process-lifetime exclusive lock before opening this DB.
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        let version: u32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            version <= SCHEMA_VERSION,
            "unsupported operation journal version"
        );
        let tx = db.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS operations (
            id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
            is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            finished_at INTEGER, action TEXT NOT NULL DEFAULT 'unknown', actor TEXT);
            CREATE INDEX IF NOT EXISTS operation_boxes ON operations(box_id, is_create);",
        )?;
        if version < 2 {
            ensure_v2_columns(&tx)?;
            tx.execute(
                "UPDATE operations SET action = 'create' WHERE is_create = 1 AND action = 'unknown'",
                [],
            )?;
        }
        if version < 3 {
            migrate_seconds_timestamps_to_ms(&tx)?;
        }
        if version < 4 {
            ensure_v4_columns(&tx)?;
        }
        if version < SCHEMA_VERSION {
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        let interrupted = tx
            .prepare("SELECT response FROM operations WHERE running=1")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for raw in interrupted {
            let mut op: Operation = serde_json::from_str(&raw)?;
            op.state = OperationState::Unknown;
            op.error = Some(ApiError { code: "interrupted".into(), message: "Host service stopped before recording the outcome. Inspect the box; this operation will not be replayed.".into() });
            tx.execute(
                "UPDATE operations SET running=0,response=?2 WHERE id=?1",
                params![op.id, serde_json::to_string(&op)?],
            )?;
        }
        tx.commit()?;
        Ok(Self(db))
    }

    fn fingerprint(request: &Submit) -> Result<Vec<u8>> {
        Ok(Sha256::digest(serde_json::to_vec(&request.action)?).to_vec())
    }

    pub fn existing(&self, request: &Submit) -> Result<Option<Operation>> {
        let found = self
            .0
            .query_row(
                "SELECT fingerprint,response FROM operations WHERE id=?1",
                [&request.id],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((fingerprint, response)) = found {
            if fingerprint != Self::fingerprint(request)? {
                return Err(Conflict.into());
            }
            return Ok(Some(serde_json::from_str(&response)?));
        }
        Ok(None)
    }

    pub fn count(&self) -> Result<u32> {
        Ok(self
            .0
            .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))?)
    }

    /// Drops finished rows using `running = 0` and `created_at` only; operation state in
    /// `response` is not consulted. Live boxes are protected via `is_create = 1` and `box_id`.
    pub fn prune_terminal(&mut self, now: i64, live_box_ids: &[&str]) -> Result<u32> {
        let cutoff = now.saturating_sub(RETENTION_SECS * 1000);
        let tx = self.0.transaction()?;
        let deleted = if live_box_ids.is_empty() {
            tx.execute(
                "DELETE FROM operations WHERE id IN (
                    SELECT o.id FROM operations o
                    WHERE o.running = 0
                      AND o.created_at < ?1
                      AND o.id NOT IN (
                        SELECT id FROM operations
                        WHERE running = 0
                        ORDER BY created_at DESC, id DESC
                        LIMIT ?2
                      )
                )",
                params![cutoff, TERMINAL_KEEP_COUNT],
            )?
        } else {
            let placeholders = std::iter::repeat_n("?", live_box_ids.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "DELETE FROM operations WHERE id IN (
                    SELECT o.id FROM operations o
                    WHERE o.running = 0
                      AND o.created_at < ?1
                      AND o.id NOT IN (
                        SELECT id FROM operations
                        WHERE running = 0
                        ORDER BY created_at DESC, id DESC
                        LIMIT ?2
                      )
                      AND NOT (o.is_create = 1 AND o.box_id IN ({placeholders}))
                )"
            );
            let mut sql_params: Vec<&dyn rusqlite::ToSql> = vec![&cutoff, &TERMINAL_KEEP_COUNT];
            for id in live_box_ids {
                sql_params.push(id);
            }
            tx.execute(&sql, sql_params.as_slice())?
        };
        tx.commit()?;
        Ok(deleted as u32)
    }

    fn make_room(&mut self, now: i64, live_box_ids: &[&str]) -> Result<()> {
        if self.count()? < HIGH_WATER_MARK {
            return Ok(());
        }
        self.prune_terminal(now, live_box_ids)?;
        if self.count()? >= HIGH_WATER_MARK {
            return Err(JournalFull.into());
        }
        Ok(())
    }

    pub fn accept(
        &mut self,
        request: &Submit,
        live_box_ids: &[&str],
        actor: Option<&str>,
    ) -> Result<Operation> {
        let now = unix_now_ms();
        self.make_room(now, live_box_ids)?;
        let op = Operation {
            id: request.id.clone(),
            box_id: request
                .action
                .box_id()
                .map(str::to_owned)
                .unwrap_or_else(new_id),
            state: OperationState::Running,
            result: None,
            error: None,
        };
        self.0.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,action,created_at,actor)
             VALUES (?1,?2,?3,?4,1,?5,?6,?7,?8)",
            params![
                op.id,
                Self::fingerprint(request)?,
                op.box_id,
                matches!(request.action, Action::Create { .. }),
                serde_json::to_string(&op)?,
                launch_action(&request.action),
                now,
                actor,
            ],
        )?;
        Ok(op)
    }

    pub fn get(&self, id: &str) -> Result<Option<Operation>> {
        let raw: Option<String> = self
            .0
            .query_row("SELECT response FROM operations WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        raw.map(|r| serde_json::from_str(&r).map_err(Into::into))
            .transpose()
    }

    /// Check before replay/fingerprint comparison. A new ID is available to any
    /// caller; existing IDs (including deleted boxes) remain bound to their actor.
    /// Legacy NULL actors are accessible only through the owner's bypass.
    pub fn allows_actor(&self, id: &str, actor: &str) -> Result<bool> {
        Ok(self.0.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM operations
             WHERE id=?1 AND (actor IS NULL OR actor != ?2))",
            params![id, actor],
            |row| row.get(0),
        )?)
    }

    pub fn finish(&mut self, op: &Operation) -> Result<()> {
        ensure!(
            op.state != OperationState::Running,
            "cannot finish a running operation"
        );
        ensure!(
            self.0.execute(
                "UPDATE operations SET running=0,response=?2,finished_at=?3 WHERE id=?1 AND running=1",
                params![op.id, serde_json::to_string(op)?, unix_now_ms()]
            )? == 1,
            "operation not running"
        );
        Ok(())
    }

    pub(crate) fn last_launch_ms(&self, box_id: &str) -> Result<Option<(String, f64)>> {
        let mut statement = self.0.prepare(
            "SELECT action, created_at, finished_at, response
             FROM operations
             WHERE box_id = ?1 AND running = 0 AND action IN ('create', 'start', 'restore')
             ORDER BY COALESCE(finished_at, created_at) DESC, created_at DESC, id DESC",
        )?;
        let rows = statement.query_map([box_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (action, created_at, finished_at, response) = row?;
            let Ok(operation) = serde_json::from_str::<Operation>(&response) else {
                continue;
            };
            if operation.state != OperationState::Succeeded {
                continue;
            }
            let Some(total_ms) = launch_total_ms(created_at, finished_at, &response) else {
                continue;
            };
            return Ok(Some((action, total_ms)));
        }
        Ok(None)
    }

    pub fn owns(&self, id: &str) -> Result<bool> {
        Ok(self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE box_id=?1 AND is_create=1)",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn managed_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .0
            .prepare("SELECT box_id FROM operations WHERE is_create=1 ORDER BY box_id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn create_times(&self) -> Result<BTreeMap<String, i64>> {
        Ok(self
            .0
            .prepare("SELECT box_id, created_at FROM operations WHERE is_create=1")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn recent(&self, limit: usize) -> Result<Vec<StoredOperation>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self.0.prepare(
            "SELECT id, box_id, is_create, created_at, response
             FROM operations ORDER BY created_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut operations = Vec::new();
        for row in rows {
            let (id, box_id, is_create, created_at, response) = row?;
            let Ok(operation) = serde_json::from_str(&response) else {
                continue;
            };
            operations.push(StoredOperation {
                id,
                box_id,
                is_create: is_create != 0,
                created_at,
                operation,
            });
        }
        Ok(operations)
    }
}

fn unix_now_ms() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

fn journal_timestamps_are_milliseconds(created_at: i64) -> bool {
    created_at >= TIMESTAMP_MS_THRESHOLD
}

fn launch_total_ms(created_at: i64, finished_at: Option<i64>, response: &str) -> Option<f64> {
    if let Some(finished_at) = finished_at
        && finished_at > created_at
    {
        let scale = if journal_timestamps_are_milliseconds(created_at) {
            1.0
        } else {
            1000.0
        };
        return Some((finished_at - created_at) as f64 * scale);
    }
    legacy_launch_total_ms(created_at, response)
}

fn legacy_launch_total_ms(created_at: i64, response: &str) -> Option<f64> {
    let value: serde_json::Value = serde_json::from_str(response).ok()?;
    if let Some(finished_at) = value.get("finished_at").and_then(|v| v.as_i64())
        && finished_at > created_at
    {
        return Some((finished_at - created_at) as f64 * 1000.);
    }
    if let Some(steps) = value.get("steps").and_then(|v| v.as_array()) {
        let mut finish: Option<i64> = None;
        for step in steps {
            let Some(step_finished) = step.get("finished_at").and_then(|v| v.as_i64()) else {
                continue;
            };
            if step_finished > created_at {
                finish = Some(finish.map_or(step_finished, |seen| seen.max(step_finished)));
            }
        }
        if let Some(finished_at) = finish {
            return Some((finished_at - created_at) as f64 * 1000.);
        }
    }
    let launch = value.get("launch")?;
    let started = launch.get("started_at").and_then(|v| v.as_i64())?;
    let finished = launch.get("finished_at").and_then(|v| v.as_i64())?;
    if finished > started && started >= created_at {
        return Some((finished - started) as f64 * 1000.);
    }
    None
}

fn launch_action(action: &Action) -> &'static str {
    match action {
        Action::Create { .. } => "create",
        Action::Start { .. } => "start",
        Action::Restore { .. } => "restore",
        _ => "other",
    }
}

pub(crate) struct StoredOperation {
    pub id: String,
    pub box_id: String,
    pub is_create: bool,
    pub created_at: i64,
    pub operation: Operation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_api::{Action, OperationState, Outcome, Submit, new_id};

    #[test]
    fn acceptance_is_durable_and_conflicts_do_not_allocate_another_box() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let request = Submit {
            id: new_id(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "first".into(),
            },
        };
        let mut db = Journal::open(&path).unwrap();
        let op = db.accept(&request, &[], None).unwrap();
        assert_eq!(op.state, OperationState::Running);
        assert!(db.owns(&op.box_id).unwrap());
        assert!(!db.owns(&new_id()).unwrap());
        assert_eq!(db.existing(&request).unwrap().unwrap().box_id, op.box_id);
        let changed = Submit {
            id: request.id.clone(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "second".into(),
            },
        };
        assert!(db.existing(&changed).is_err());
        drop(db);
        let db = Journal::open(&path).unwrap();
        let recovered = db.existing(&request).unwrap().unwrap();
        assert_eq!(recovered.box_id, op.box_id);
        assert_eq!(recovered.state, OperationState::Unknown);
        assert_eq!(db.managed_ids().unwrap(), vec![op.box_id]);
    }

    #[test]
    fn finished_requests_survive_restart_and_exec_secrets_are_not_recorded() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let request = Submit {
            id: new_id(),
            action: Action::Exec {
                id: new_id(),
                request: kiln_api::ExecRequest {
                    argv: vec!["/bin/echo".into(), "secret-command-7183".into()],
                    cwd: None,
                    env: [("TOKEN".into(), "secret-env-9372".into())].into(),
                    timeout_ms: 100,
                },
            },
        };
        let mut op = db.accept(&request, &[], None).unwrap();
        op.state = OperationState::Succeeded;
        op.result = Some(Outcome::Exec(kiln_api::ExecResult {
            stdout: vec![0, 255, 17],
            stderr: vec![42],
            exit_code: Some(37),
            truncated: false,
            timed_out: false,
        }));
        db.finish(&op).unwrap();
        drop(db);
        let db = Journal::open(&path).unwrap();
        let read = db.get(&request.id).unwrap().unwrap();
        assert_eq!(read.state, OperationState::Succeeded);
        assert_eq!(
            serde_json::to_value(read.result).unwrap(),
            serde_json::to_value(op.result).unwrap()
        );
        drop(db);
        let bytes = std::fs::read(path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret-command-7183"));
        assert!(!String::from_utf8_lossy(&bytes).contains("secret-env-9372"));
    }

    fn insert_terminal(
        db: &Connection,
        id: &str,
        box_id: &str,
        is_create: bool,
        created_at: i64,
        state: OperationState,
    ) {
        let op = Operation {
            id: id.into(),
            box_id: box_id.into(),
            state,
            result: None,
            error: None,
        };
        db.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at)
             VALUES (?1,zeroblob(32),?2,?3,0,?4,?5)",
            params![
                id,
                box_id,
                is_create as i64,
                serde_json::to_string(&op).unwrap(),
                created_at,
            ],
        )
        .unwrap();
    }

    fn create_user_version_one_db(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE operations (
                id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
                is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX operation_boxes ON operations(box_id, is_create);
             PRAGMA user_version=1;",
        )
        .unwrap();
        db
    }

    fn insert_running(db: &Connection, id: &str, box_id: &str, created_at: i64) {
        let op = Operation {
            id: id.into(),
            box_id: box_id.into(),
            state: OperationState::Running,
            result: None,
            error: None,
        };
        db.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at)
             VALUES (?1,zeroblob(32),?2,0,1,?3,?4)",
            params![id, box_id, serde_json::to_string(&op).unwrap(), created_at],
        )
        .unwrap();
    }

    #[test]
    fn accept_prunes_past_high_water_when_old_terminal_rows_exist() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let old = unix_now_ms() - RETENTION_SECS * 1000 - 60_000;
        for index in 0..HIGH_WATER_MARK {
            insert_terminal(
                &db.0,
                &format!("{:032x}", index),
                &new_id(),
                false,
                old,
                OperationState::Succeeded,
            );
        }
        assert_eq!(db.count().unwrap(), HIGH_WATER_MARK);
        let request = Submit {
            id: new_id(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "after-prune".into(),
            },
        };
        db.accept(&request, &[], None).unwrap();
        assert!(db.count().unwrap() < HIGH_WATER_MARK);
        assert!(db.existing(&request).unwrap().is_some());
    }

    #[test]
    fn prune_keeps_live_box_create_rows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let live_box = new_id();
        let old = unix_now_ms() - RETENTION_SECS * 1000 - 60_000;
        insert_terminal(
            &db.0,
            &new_id(),
            &live_box,
            true,
            old,
            OperationState::Succeeded,
        );
        assert!(db.owns(&live_box).unwrap());
        db.prune_terminal(unix_now_ms(), &[&live_box]).unwrap();
        assert!(db.owns(&live_box).unwrap());
        assert_eq!(db.managed_ids().unwrap(), vec![live_box]);
    }

    #[test]
    fn prune_keeps_running_operations() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let running_id = new_id();
        let old = unix_now_ms() - RETENTION_SECS * 1000 - 60_000;
        insert_running(&db.0, &running_id, &new_id(), old);
        db.prune_terminal(unix_now_ms(), &[]).unwrap();
        assert!(db.get(&running_id).unwrap().is_some());
        assert_eq!(
            db.get(&running_id).unwrap().unwrap().state,
            OperationState::Running
        );
    }

    #[test]
    fn fresh_journal_is_version_two_and_reopening_is_a_noop() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        drop(Journal::open(&path).unwrap());
        let version: u32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(table_has_column(&Connection::open(&path).unwrap(), "finished_at").unwrap());
        assert!(table_has_column(&Connection::open(&path).unwrap(), "action").unwrap());
        assert!(table_has_column(&Connection::open(&path).unwrap(), "actor").unwrap());
        drop(Journal::open(&path).unwrap());
        let version: u32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn v1_upgrade_leaves_legacy_finished_at_null() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let conn = Connection::open(&path).unwrap();
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
             VALUES ('op1', x'00', 'box1', 1, 0, '{\"state\":\"succeeded\"}', 100)",
            [],
        )
        .unwrap();
        drop(conn);
        drop(Journal::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        let finished_at: Option<i64> = conn
            .query_row(
                "SELECT finished_at FROM operations WHERE id='op1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(finished_at.is_none());
        let created_at: i64 = conn
            .query_row(
                "SELECT created_at FROM operations WHERE id='op1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(created_at, 100_000);
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn v1_journal_shape_uses_created_at_and_running_for_retention() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("operations.sqlite");
        let live_box = new_id();
        let create_id = new_id();
        let stale_exec = format!("{:032x}", 0);
        let running_id = new_id();
        let recent_exec = new_id();
        let old = unix_now_ms() - RETENTION_SECS * 1000 - 3_600_000;
        let recent = unix_now_ms() - 3_600_000;
        {
            let db = create_user_version_one_db(&path);
            for index in 1..=TERMINAL_KEEP_COUNT {
                insert_terminal(
                    &db,
                    &format!("{:032x}", index),
                    &new_id(),
                    false,
                    old,
                    OperationState::Succeeded,
                );
            }
            insert_terminal(
                &db,
                &create_id,
                &live_box,
                true,
                old,
                OperationState::Succeeded,
            );
            insert_terminal(
                &db,
                &stale_exec,
                &live_box,
                false,
                old,
                OperationState::Succeeded,
            );
            insert_running(&db, &running_id, &live_box, old);
            insert_terminal(
                &db,
                &recent_exec,
                &new_id(),
                false,
                recent,
                OperationState::Succeeded,
            );
        }
        let mut journal = Journal::open(&path).unwrap();
        let version: u32 = journal
            .0
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        journal.prune_terminal(unix_now_ms(), &[&live_box]).unwrap();
        assert!(journal.get(&create_id).unwrap().is_some());
        assert!(journal.get(&running_id).unwrap().is_some());
        assert!(journal.get(&recent_exec).unwrap().is_some());
        assert!(journal.get(&stale_exec).unwrap().is_none());
        assert!(journal.owns(&live_box).unwrap());
    }

    #[test]
    fn v2_finish_records_measured_launch_duration() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let request = Submit {
            id: new_id(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "box".into(),
            },
        };
        let mut op = db.accept(&request, &[], None).unwrap();
        op.state = OperationState::Succeeded;
        std::thread::sleep(std::time::Duration::from_secs(1));
        db.finish(&op).unwrap();
        let conn = Connection::open(&path).unwrap();
        let finished_at: i64 = conn
            .query_row(
                "SELECT finished_at FROM operations WHERE id=?1",
                [&op.id],
                |row| row.get(0),
            )
            .unwrap();
        let created_at: i64 = conn
            .query_row(
                "SELECT created_at FROM operations WHERE id=?1",
                [&op.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(finished_at > created_at);
        let measured = db.last_launch_ms(&op.box_id).unwrap().unwrap();
        assert!(measured.1 > 0.);
    }

    #[test]
    fn millisecond_journal_timestamps_report_direct_duration() {
        let created = 1_700_000_000_000_i64;
        let finished = created + 842;
        assert_eq!(launch_total_ms(created, Some(finished), "{}"), Some(842.));
    }

    #[test]
    fn v2_seconds_journal_upgrades_to_millisecond_timestamps() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version=2;
             CREATE TABLE operations (
               id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
               is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
               created_at INTEGER NOT NULL, finished_at INTEGER, action TEXT NOT NULL DEFAULT 'unknown');",
        )
        .unwrap();
        let base = 1_700_000_000_i64;
        for index in 0..230 {
            let id = format!("{:032x}", index + 1);
            let box_id = format!("{:032x}", index + 1000);
            let response = format!(
                r#"{{"id":"{id}","box_id":"{box_id}","state":"succeeded","result":null,"error":null}}"#
            );
            let finished = if index % 3 == 0 {
                Some(base + index as i64 + 2)
            } else {
                None
            };
            conn.execute(
                "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at,finished_at,action)
                 VALUES (?1,x'00',?2,1,0,?5,?3,?4,'create')",
                params![id, box_id, base + index as i64, finished, response],
            )
            .unwrap();
        }
        drop(conn);
        let db = Journal::open(&path).unwrap();
        let version: u32 =
            db.0.query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let (min_created, max_created): (i64, i64) =
            db.0.query_row(
                "SELECT min(created_at), max(created_at) FROM operations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(min_created >= TIMESTAMP_MS_THRESHOLD);
        assert!(max_created >= TIMESTAMP_MS_THRESHOLD);
        let sample_id = format!("{:032x}", 1);
        let (created, finished): (i64, i64) =
            db.0.query_row(
                "SELECT created_at, finished_at FROM operations WHERE id=?1",
                [&sample_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(created, base * 1000);
        assert_eq!(finished, (base + 2) * 1000);
        let measured = db
            .last_launch_ms("000000000000000000000000000003e8")
            .unwrap()
            .unwrap();
        assert!((measured.1 - 2000.).abs() < 0.001, "got {}", measured.1);
    }

    #[test]
    fn legacy_response_timestamps_derive_launch_interval() {
        let created_at = 1_000;
        let response = serde_json::json!({
            "state": "succeeded",
            "finished_at": 1_004
        })
        .to_string();
        assert_eq!(legacy_launch_total_ms(created_at, &response), Some(4000.));
        let steps = serde_json::json!({
            "state": "succeeded",
            "steps": [
                {"finished_at": 1_002},
                {"finished_at": 1_005}
            ]
        })
        .to_string();
        assert_eq!(legacy_launch_total_ms(created_at, &steps), Some(5000.));
    }

    #[test]
    fn accept_refuses_when_still_full_after_prune() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let recent = unix_now_ms() - 60_000;
        for index in 0..HIGH_WATER_MARK {
            insert_terminal(
                &db.0,
                &format!("{:032x}", index),
                &new_id(),
                false,
                recent,
                OperationState::Succeeded,
            );
        }
        let request = Submit {
            id: new_id(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "blocked".into(),
            },
        };
        assert!(db.accept(&request, &[], None).is_err());
    }

    #[test]
    fn v3_upgrade_adds_actor_column_with_null_on_legacy_rows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version=3;
             CREATE TABLE operations (
               id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
               is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
               created_at INTEGER NOT NULL, finished_at INTEGER,
               action TEXT NOT NULL DEFAULT 'unknown');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at,action)
             VALUES ('op1', x'00', 'box1', 1, 0, '{\"state\":\"succeeded\"}', 1700000000000, 'create')",
            [],
        )
        .unwrap();
        drop(conn);
        let mut db = Journal::open(&path).unwrap();
        assert_eq!(
            db.0.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        let actor: Option<String> =
            db.0.query_row("SELECT actor FROM operations WHERE id='op1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(actor.is_none());
        let request = Submit {
            id: new_id(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "owned".into(),
            },
        };
        let owner = "0123456789abcdef0123456789abcdef";
        let op = db.accept(&request, &[], Some(owner)).unwrap();
        let actor: String =
            db.0.query_row(
                "SELECT actor FROM operations WHERE id=?1",
                [&op.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actor, owner);
    }
}
