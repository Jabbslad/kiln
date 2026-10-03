use std::{
    fmt,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Config {
    pub public_origin: String,
    pub github_client_id: String,
    pub github_secret_file: PathBuf,
    pub google_client_id: String,
    pub google_secret_file: PathBuf,
    pub signing_key_file: PathBuf,
    pub signing_key_id: String,
    pub retiring_jwks_file: Option<PathBuf>,
    pub trusted_proxy_loopback: bool,
    pub now: fn() -> i64,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("public_origin", &self.public_origin)
            .field("github_client_id", &self.github_client_id)
            .field("google_client_id", &self.google_client_id)
            .field("signing_key_id", &self.signing_key_id)
            .finish_non_exhaustive()
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
