use anyhow::{Result, ensure};
use kiln_api::{Action, ApiError, Operation, OperationState, Submit, new_id};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug)]
pub struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request ID already used for a different action")
    }
}
impl std::error::Error for Conflict {}

pub struct Journal(Connection);

const SCHEMA_VERSION: u32 = 2;

fn table_has_column(connection: &Connection, column: &str) -> Result<bool> {
    let mut statement = connection.prepare("PRAGMA table_info(operations)")?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(names.filter_map(Result::ok).any(|name| name == column))
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
            finished_at INTEGER, action TEXT NOT NULL DEFAULT 'unknown');
            CREATE INDEX IF NOT EXISTS operation_boxes ON operations(box_id, is_create);",
        )?;
        if version < SCHEMA_VERSION {
            ensure_v2_columns(&tx)?;
            tx.execute(
                "UPDATE operations SET action = 'create' WHERE is_create = 1 AND action = 'unknown'",
                [],
            )?;
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

    pub fn accept(&mut self, request: &Submit) -> Result<Operation> {
        let count: u32 = self
            .0
            .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))?;
        ensure!(
            count < 10_000,
            "operation journal full; archive this installation before accepting more work"
        );
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
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,action) VALUES (?1,?2,?3,?4,1,?5,?6)",
            params![
                op.id,
                Self::fingerprint(request)?,
                op.box_id,
                matches!(request.action, Action::Create { .. }),
                serde_json::to_string(&op)?,
                launch_action(&request.action),
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

    pub fn finish(&mut self, op: &Operation) -> Result<()> {
        ensure!(
            op.state != OperationState::Running,
            "cannot finish a running operation"
        );
        ensure!(
            self.0.execute(
                "UPDATE operations SET running=0,response=?2,finished_at=unixepoch() WHERE id=?1 AND running=1",
                params![op.id, serde_json::to_string(op)?]
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

fn launch_total_ms(created_at: i64, finished_at: Option<i64>, response: &str) -> Option<f64> {
    if let Some(finished_at) = finished_at
        && finished_at > created_at
    {
        return Some((finished_at - created_at) as f64 * 1000.);
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
        let op = db.accept(&request).unwrap();
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
        let mut op = db.accept(&request).unwrap();
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
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
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
        let mut op = db.accept(&request).unwrap();
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
}
