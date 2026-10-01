use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u16 = 1;
pub const VSOCK_PORT: u32 = 1024;
pub const HOST_CID: u32 = 2;
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;
pub const MAX_OUTPUT_SIZE: usize = 64 * 1024;
// Public, non-secret preparation identity. Only explicit shared-identity images
// retain it for workloads; normal clones replace it before becoming ready.
pub const PREPARATION_MACHINE_ID: &str = "11111111111111111111111111111111";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello { version: u16 },
    Initialize(InitializeRequest),
    Exec(ExecRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeRequest {
    pub hostname: String,
    pub machine_id: String,
    pub entropy: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Hello { version: u16, initialized: bool },
    Initialized { version: u16 },
    Exec(ExecResult),
    Error(ProtocolError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    NotInitialized,
    AlreadyInitialized,
    InvalidRequest,
    InitializationFailed,
    ExecutionFailed,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("transport error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame length {length} exceeds limit {limit}")]
    TooLarge { length: usize, limit: usize },
    #[error("invalid JSON frame: {0}")]
    Json(#[from] serde_json::Error),
}

pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = reader.read_u32().await? as usize;
    if length > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge {
            length,
            limit: MAX_FRAME_SIZE,
        });
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge {
            length: payload.len(),
            limit: MAX_FRAME_SIZE,
        });
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}
