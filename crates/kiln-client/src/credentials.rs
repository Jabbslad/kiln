use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedToken {
    pub token: String,
    pub expires_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub issuer: String,
    pub access_token: String,
    pub access_expires_at: u64,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_key_file: Option<PathBuf>,
    #[serde(default)]
    pub server_tokens: BTreeMap<String, CachedToken>,
}

#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn parent(&self) -> &Path {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }

    fn secure_parent(&self) -> Result<()> {
        let parent = self.parent();
        if parent.exists() {
            reject_link(parent)?;
        } else {
            let mut b = fs::DirBuilder::new();
            b.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            b.create(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let m = fs::symlink_metadata(parent)?;
            ensure!(
                m.is_dir() && m.uid() == unsafe { libc::geteuid() },
                "credential directory is not owned by current user"
            );
            ensure!(
                m.permissions().mode() & 0o077 == 0,
                "credential directory is accessible by other users"
            );
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Credentials>> {
        self.secure_parent()?;
        if !self.path.try_exists()? && fs::symlink_metadata(&self.path).is_err() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&read_private(
            &self.path,
            1024 * 1024,
        )?)?))
    }

    pub fn save(&self, value: &Credentials) -> Result<()> {
        self.secure_parent()?;
        if fs::symlink_metadata(&self.path).is_ok() {
            reject_link(&self.path)?;
        }
        let mut tmp = tempfile::NamedTempFile::new_in(self.parent())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tmp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        tmp.write_all(&serde_json::to_vec(value)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        #[cfg(unix)]
        fs::File::open(self.parent())?.sync_all()?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        self.secure_parent()?;
        if fs::symlink_metadata(&self.path).is_ok() {
            reject_link(&self.path)?;
            fs::remove_file(&self.path)?;
            #[cfg(unix)]
            fs::File::open(self.parent())?.sync_all()?;
        }
        Ok(())
    }

    pub async fn acquire(&self) -> Result<fs::File> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.lock()).await?
    }

    pub fn lock(&self) -> Result<fs::File> {
        self.secure_parent()?;
        let p = self.parent().join(".credentials.lock");
        if fs::symlink_metadata(&p).is_ok() {
            reject_link(&p)?;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let f = options.open(p)?;
        check_file(&f, 1024)?;
        f.lock().context("could not lock credentials")?;
        Ok(f)
    }
}

fn reject_link(path: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "refusing symlink in credential storage"
    );
    Ok(())
}

fn check_file(file: &fs::File, limit: u64) -> Result<()> {
    let m = file.metadata()?;
    ensure!(m.is_file() && m.len() <= limit, "invalid credential file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "credential file is not private"
        );
    }
    Ok(())
}

pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    reject_link(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    check_file(&file, limit)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "credential file too large");
    Ok(bytes)
}

/// Create a new secret file; never overwrite an existing key.
pub fn write_secret(path: &Path, value: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)?
    };
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn read_secret(path: &Path) -> Result<String> {
    let text = String::from_utf8(read_private(path, 8192)?)?;
    let token = text.trim();
    ensure!(
        !token.is_empty()
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~+/=".contains(&b)),
        "invalid credential"
    );
    Ok(token.to_owned())
}
