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

impl Journal {
    // The host holds a process-lifetime exclusive lock before opening this DB.
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        let version: u32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(version <= 1, "unsupported operation journal version");
        let tx = db.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS operations (
            id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
            is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()));
            CREATE INDEX IF NOT EXISTS operation_boxes ON operations(box_id, is_create);
            PRAGMA user_version=1;",
        )?;
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
        let cutoff = now.saturating_sub(RETENTION_SECS);
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

    pub fn accept(&mut self, request: &Submit, live_box_ids: &[&str]) -> Result<Operation> {
        let now = unix_now();
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
        self.0.execute("INSERT INTO operations(id,fingerprint,box_id,is_create,running,response) VALUES (?1,?2,?3,?4,1,?5)", params![op.id, Self::fingerprint(request)?, op.box_id, matches!(request.action, Action::Create { .. }), serde_json::to_string(&op)?])?;
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
                "UPDATE operations SET running=0,response=?2 WHERE id=?1 AND running=1",
                params![op.id, serde_json::to_string(op)?]
            )? == 1,
            "operation not running"
        );
        Ok(())
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

fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
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
        let op = db.accept(&request, &[]).unwrap();
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
        let mut op = db.accept(&request, &[]).unwrap();
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
        let old = unix_now() - RETENTION_SECS - 60;
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
        db.accept(&request, &[]).unwrap();
        assert!(db.count().unwrap() < HIGH_WATER_MARK);
        assert!(db.existing(&request).unwrap().is_some());
    }

    #[test]
    fn prune_keeps_live_box_create_rows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let live_box = new_id();
        let old = unix_now() - RETENTION_SECS - 60;
        insert_terminal(
            &db.0,
            &new_id(),
            &live_box,
            true,
            old,
            OperationState::Succeeded,
        );
        assert!(db.owns(&live_box).unwrap());
        db.prune_terminal(unix_now(), &[&live_box]).unwrap();
        assert!(db.owns(&live_box).unwrap());
        assert_eq!(db.managed_ids().unwrap(), vec![live_box]);
    }

    #[test]
    fn prune_keeps_running_operations() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let running_id = new_id();
        let old = unix_now() - RETENTION_SECS - 60;
        insert_running(&db.0, &running_id, &new_id(), old);
        db.prune_terminal(unix_now(), &[]).unwrap();
        assert!(db.get(&running_id).unwrap().is_some());
        assert_eq!(
            db.get(&running_id).unwrap().unwrap().state,
            OperationState::Running
        );
    }

    #[test]
    fn idempotent_retry_dedupes_and_changed_body_conflicts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let request = Submit {
            id: new_id(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "same".into(),
            },
        };
        let op = db.accept(&request, &[]).unwrap();
        assert_eq!(db.existing(&request).unwrap().unwrap().box_id, op.box_id);
        let changed = Submit {
            id: request.id.clone(),
            action: Action::Create {
                template: "ubuntu".into(),
                name: "different".into(),
            },
        };
        assert!(db.existing(&changed).is_err());
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
        let old = unix_now() - RETENTION_SECS - 3600;
        let recent = unix_now() - 3600;
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
        assert_eq!(version, 1);
        let columns: Vec<String> = journal
            .0
            .prepare("SELECT name FROM pragma_table_info('operations') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "id",
                "fingerprint",
                "box_id",
                "is_create",
                "running",
                "response",
                "created_at"
            ]
        );
        journal.prune_terminal(unix_now(), &[&live_box]).unwrap();
        assert!(journal.get(&create_id).unwrap().is_some());
        assert!(journal.get(&running_id).unwrap().is_some());
        assert!(journal.get(&recent_exec).unwrap().is_some());
        assert!(journal.get(&stale_exec).unwrap().is_none());
        assert!(journal.owns(&live_box).unwrap());
    }

    #[test]
    fn accept_refuses_when_still_full_after_prune() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ops.sqlite");
        let mut db = Journal::open(&path).unwrap();
        let recent = unix_now() - 60;
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
        assert!(db.accept(&request, &[]).is_err());
    }
}
