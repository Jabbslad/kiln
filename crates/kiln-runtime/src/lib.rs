pub mod firecracker;
pub mod guest;
pub mod host;
pub mod image;
pub mod isolation;
pub mod network;
pub mod process;
pub mod runtime;
pub mod storage;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Protocol(#[from] kiln_protocol::FrameError),
}
