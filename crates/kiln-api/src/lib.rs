//! Portable, public management contract. No runtime records or host paths.
pub mod access;
pub mod auth;
pub use access::may_access_box;
pub use auth::{AuthGrant, Caller, Role, Scope, ServerClaims};
pub use kiln_protocol::{ExecRequest, ExecResult, SshReady, release_version, valid_ssh_public_key};
use serde::{Deserialize, Serialize};

pub const MAX_REQUEST_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const SSH_UPGRADE: &str = "kiln-ssh";
pub const SSH_KEY_HEADER: &str = "x-kiln-ssh-key";
/// Loopback host hint: gateway sets this to the authenticated central JWT `sub`.
pub const OPERATION_ACTOR_HEADER: &str = "x-kiln-operation-actor";

/// Token bytes never appear in errors. Unix enforces owner-only mode here.
pub fn read_token(path: &std::path::Path) -> std::io::Result<String> {
    use std::{
        fs,
        io::{Error, ErrorKind, Read},
    };
    let invalid = || {
        Error::new(
            ErrorKind::InvalidData,
            "token needs a private regular file containing 64 hexadecimal characters",
        )
    };
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid());
    }
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 128 {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    let mut token = String::new();
    file.take(129).read_to_string(&mut token)?;
    let token = token.trim();
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    Ok(token.to_owned())
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 63
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Create {
        template: String,
        name: String,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Stop {
        id: String,
        #[serde(default)]
        force: bool,
    },
    Start {
        id: String,
    },
    Delete {
        id: String,
    },
    Exec {
        id: String,
        request: ExecRequest,
    },
    /// Capture a checkpoint of a running or paused box.
    Checkpoint {
        id: String,
    },
    /// Restore a checkpoint onto the same box.
    Restore {
        id: String,
        checkpoint: String,
        /// Memory and disk rewind. External systems may replay; required.
        #[serde(default)]
        acknowledge_external_state_replay: bool,
    },
}

impl Action {
    pub fn box_id(&self) -> Option<&str> {
        match self {
            Self::Create { .. } => None,
            Self::Pause { id }
            | Self::Resume { id }
            | Self::Stop { id, .. }
            | Self::Start { id }
            | Self::Delete { id }
            | Self::Exec { id, .. }
            | Self::Checkpoint { id }
            | Self::Restore { id, .. } => Some(id),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.box_id().is_some_and(|id| !valid_id(id)) {
            return Err("invalid box ID");
        }
        match self {
            Self::Create { template, name } if !valid_alias(template) || !valid_alias(name) => {
                return Err(
                    "template and name must be 1..63 ASCII letters, digits, hyphens or underscores",
                );
            }
            Self::Exec { request, .. }
                if (!(1..=3_600_000).contains(&request.timeout_ms)
                    || request.argv.is_empty()
                    || request.argv.len() > 256
                    || request.argv[0].is_empty()
                    || request.argv.iter().any(|a| a.contains('\0'))
                    || request
                        .cwd
                        .as_ref()
                        .is_some_and(|p| !p.starts_with('/') || p.contains('\0'))
                    || request.env.len() > 128
                    || request.env.iter().any(|(k, v)| {
                        k.is_empty() || k.contains(['=', '\0']) || v.contains('\0')
                    })) =>
            {
                return Err("invalid exec arguments, environment, cwd or timeout (1..3600000 ms)");
            }
            Self::Restore {
                checkpoint,
                acknowledge_external_state_replay,
                ..
            } => {
                if !valid_id(checkpoint) {
                    return Err("invalid checkpoint ID");
                }
                if !acknowledge_external_state_replay {
                    return Err(
                        "restore rewinds memory and disk, not external systems; pass --acknowledge-external-state-replay",
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Submit {
    pub id: String,
    pub action: Action,
}

pub const DEFAULT_OPERATION_WAIT_SECONDS: u64 = 300;
pub const MAX_OPERATION_WAIT_SECONDS: u64 = 3605;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationSubmitQuery {
    pub wait: bool,
    pub wait_seconds: u64,
}

impl OperationSubmitQuery {
    pub fn parse(path: &str, query: Option<&str>) -> Result<Self, &'static str> {
        if query.is_none() {
            return Ok(Self {
                wait: false,
                wait_seconds: DEFAULT_OPERATION_WAIT_SECONDS,
            });
        }
        if path != "/v1/operations" {
            return Err("query parameters are not accepted");
        }
        let mut wait = false;
        let mut wait_seconds = None;
        for pair in query.unwrap().split('&') {
            if pair.is_empty() {
                return Err("invalid query parameter");
            }
            let (key, value) = pair.split_once('=').ok_or("invalid query parameter")?;
            match key {
                "wait" => {
                    wait = match value {
                        "true" | "1" => true,
                        "false" | "0" => false,
                        _ => return Err("invalid wait query parameter"),
                    };
                }
                "wait_seconds" => {
                    wait_seconds = Some(
                        value
                            .parse::<u64>()
                            .map_err(|_| "invalid wait_seconds query parameter")?,
                    );
                }
                _ => return Err("unknown query parameter"),
            }
        }
        let wait_seconds = wait_seconds.unwrap_or(DEFAULT_OPERATION_WAIT_SECONDS);
        if !(1..=MAX_OPERATION_WAIT_SECONDS).contains(&wait_seconds) {
            return Err("wait_seconds must be between 1 and 3605");
        }
        Ok(Self { wait, wait_seconds })
    }

    pub fn validate_for_action(&self, action: &Action) -> Result<(), &'static str> {
        if self.wait && !matches!(action, Action::Create { .. }) {
            return Err("wait is only supported for create");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoxView {
    pub id: String,
    pub name: String,
    pub state: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateView {
    pub name: String,
    pub memory_mib: u32,
    pub vcpus: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointView {
    pub id: String,
    pub box_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Running,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    Box(BoxView),
    Deleted { id: String },
    Exec(ExecResult),
    Checkpoint(CheckpointView),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub box_id: String,
    pub state: OperationState,
    pub result: Option<Outcome>,
    pub error: Option<ApiError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_paths_unknown_fields_and_invalid_names() {
        assert!(
            Action::Delete {
                id: "../outside".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Action::Create {
                template: "/etc/passwd".into(),
                name: "fine".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Action::Create {
                template: "ubuntu".into(),
                name: "bad\nname".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Action>(
                r#"{"type":"create","template":"ubuntu","name":"ok","image":"/tmp/evil"}"#
            )
            .is_err()
        );
        assert!(
            Action::Create {
                template: "ubuntu-4g".into(),
                name: "my-box_2".into()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn exec_validates_both_sides_of_deadline_and_empty_command_boundaries() {
        for (timeout_ms, valid) in [(0, false), (1, true), (3_600_000, true), (3_600_001, false)] {
            let action = Action::Exec {
                id: "a".repeat(32),
                request: ExecRequest {
                    argv: vec!["/bin/true".into()],
                    cwd: None,
                    env: Default::default(),
                    timeout_ms,
                },
            };
            assert_eq!(action.validate().is_ok(), valid, "deadline {timeout_ms}");
        }
        for argv in [vec![], vec!["".into()], vec!["bad\0command".into()]] {
            assert!(
                Action::Exec {
                    id: "b".repeat(32),
                    request: ExecRequest {
                        argv,
                        cwd: None,
                        env: Default::default(),
                        timeout_ms: 10,
                    }
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn operation_submit_query_accepts_only_create_wait_parameters() {
        let create = Action::Create {
            template: "ubuntu".into(),
            name: "box".into(),
        };
        let parsed = OperationSubmitQuery::parse("/v1/operations", Some("wait=true")).unwrap();
        assert!(parsed.wait);
        assert_eq!(parsed.wait_seconds, DEFAULT_OPERATION_WAIT_SECONDS);
        parsed.validate_for_action(&create).unwrap();
        assert!(
            OperationSubmitQuery::parse("/v1/operations", Some("wait=true&wait_seconds=42"))
                .unwrap()
                .wait_seconds
                == 42
        );
        assert!(
            OperationSubmitQuery::parse("/v1/boxes", Some("wait=true")).is_err(),
            "query is limited to POST /v1/operations"
        );
        assert!(OperationSubmitQuery::parse("/v1/operations", Some("preview=1")).is_err());
        let parsed = OperationSubmitQuery::parse("/v1/operations", Some("wait=false")).unwrap();
        parsed.validate_for_action(&create).unwrap();
        assert!(
            parsed
                .validate_for_action(&Action::Delete { id: "a".repeat(32) })
                .is_ok()
        );
        assert!(
            OperationSubmitQuery::parse("/v1/operations", Some("wait=true"))
                .unwrap()
                .validate_for_action(&Action::Delete { id: "a".repeat(32) })
                .is_err()
        );
    }

    #[test]
    fn dashboard_operation_action_types_match_api_action_tags() {
        use std::collections::BTreeSet;

        let web_source = include_str!("../../../web/src/operationKinds.ts");
        let marker = "export const OPERATION_ACTION_TYPES = [";
        let rest = web_source
            .split(marker)
            .nth(1)
            .expect("web operation action type list");
        let inner = rest.split(']').next().expect("web operation action array");
        let web: BTreeSet<String> = inner
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.trim_matches(['\'', '"']).to_string())
            .collect();

        let samples = [
            Action::Create {
                template: "ubuntu".into(),
                name: "box".into(),
            },
            Action::Pause { id: "a".repeat(32) },
            Action::Resume { id: "a".repeat(32) },
            Action::Stop {
                id: "a".repeat(32),
                force: false,
            },
            Action::Start { id: "a".repeat(32) },
            Action::Delete { id: "a".repeat(32) },
            Action::Exec {
                id: "a".repeat(32),
                request: ExecRequest {
                    argv: vec!["/bin/true".into()],
                    cwd: None,
                    env: Default::default(),
                    timeout_ms: 1_000,
                },
            },
            Action::Checkpoint { id: "a".repeat(32) },
            Action::Restore {
                id: "a".repeat(32),
                checkpoint: "b".repeat(32),
                acknowledge_external_state_replay: true,
            },
        ];
        let api: BTreeSet<String> = samples
            .iter()
            .map(|action| {
                let value = serde_json::to_value(action).expect("serialize action");
                value
                    .get("type")
                    .and_then(|tag| tag.as_str())
                    .expect("action tag")
                    .to_string()
            })
            .collect();

        assert_eq!(
            web, api,
            "web OPERATION_ACTION_TYPES must match kiln_api::Action tags"
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_files_must_be_private_regular_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("token");
        std::fs::write(&path, format!("{}\n", "b".repeat(64))).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_token(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_token(&path).unwrap(), "b".repeat(64));
        symlink(&path, root.path().join("link")).unwrap();
        assert!(read_token(&root.path().join("link")).is_err());
        std::fs::write(&path, "short").unwrap();
        assert!(read_token(&path).is_err());
    }
}
