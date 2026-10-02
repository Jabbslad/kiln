use anyhow::{Result, ensure};
use kiln_api::{Action, ApiError, Operation, OperationState, Submit, new_id};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug)]
pub struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request ID already used for a different action")
    }
}
impl std::error::Error for Conflict {}

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
}
