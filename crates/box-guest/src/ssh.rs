use crate::{Agent, Initializer};
use box_protocol::{SshConnect, SshReady, read_frame, valid_ssh_public_key, write_frame};
use std::{
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

pub const HOST_KEY_DIRECTORY: &str = "/var/lib/boxd/ssh";
pub const SESSION_DIRECTORY: &str = "/run/boxd-ssh";
static SESSION_ID: AtomicU64 = AtomicU64::new(0);

pub struct SessionFiles {
    directory: PathBuf,
    authorized_keys: PathBuf,
    config: PathBuf,
}

impl SessionFiles {
    pub fn create(root: &Path, public_key: &str, host_key: &str) -> io::Result<Self> {
        if !valid_ssh_public_key(public_key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SSH public key",
            ));
        }
        fs::create_dir_all(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        let id = SESSION_ID.fetch_add(1, Ordering::Relaxed);
        let directory = root.join(format!("{}-{id}", std::process::id()));
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let authorized_keys = directory.join("authorized_keys");
        let config = directory.join("sshd_config");
        write_private(&authorized_keys, format!("{public_key}\n").as_bytes())?;
        let contents = format!(
            "HostKey {host_key}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nChallengeResponseAuthentication no\nPubkeyAuthentication yes\nAuthenticationMethods publickey\nPermitRootLogin prohibit-password\nAllowUsers root\nUsePAM yes\nLoginGraceTime 20\nMaxAuthTries 3\nMaxSessions 8\nAllowAgentForwarding no\nX11Forwarding no\nAllowTcpForwarding local\nPermitOpen localhost:* 127.0.0.1:* [::1]:*\nGatewayPorts no\nPermitTunnel no\nPermitUserEnvironment no\nStrictModes yes\nSubsystem sftp internal-sftp\n",
            authorized_keys.display()
        );
        write_private(&config, contents.as_bytes())?;
        Ok(Self {
            directory,
            authorized_keys,
            config,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn authorized_keys(&self) -> &Path {
        &self.authorized_keys
    }
    pub fn config(&self) -> &Path {
        &self.config
    }
}

impl Drop for SessionFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

pub struct HostKeys {
    directory: PathBuf,
    lock: Mutex<()>,
}

impl HostKeys {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            lock: Mutex::new(()),
        }
    }

    pub async fn public_key(&self) -> io::Result<String> {
        let _guard = self.lock.lock().await;
        fs::create_dir_all(&self.directory)?;
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        let private = self.directory.join("ssh_host_ed25519_key");
        if !private.exists() {
            let status = tokio::process::Command::new("/usr/bin/ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
                .arg(&private)
                .kill_on_drop(true)
                .status()
                .await?;
            if !status.success() {
                return Err(io::Error::other("ssh-keygen failed"));
            }
        }
        public_from_private(&private).await
    }

    pub fn private_key(&self) -> PathBuf {
        self.directory.join("ssh_host_ed25519_key")
    }
}

async fn public_from_private(private: &Path) -> io::Result<String> {
    let output = tokio::process::Command::new("/usr/bin/ssh-keygen")
        .args(["-y", "-f"])
        .arg(private)
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(io::Error::other("could not read SSH host public key"));
    }
    let key = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let key = key.trim_end_matches('\n');
    if !valid_ssh_public_key(key) {
        return Err(io::Error::other(
            "ssh-keygen returned an invalid Ed25519 key",
        ));
    }
    Ok(key.to_owned())
}

pub async fn serve_session<S, I>(mut stream: S, agent: &Agent<I>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: Initializer,
{
    let admission = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_frame::<_, SshConnect>(&mut stream),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH admission timed out"))?
    .map_err(io::Error::other)?;
    if !valid_ssh_public_key(&admission.public_key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH public key",
        ));
    }
    let (host_public_key, host_private_key) =
        tokio::time::timeout(std::time::Duration::from_secs(5), agent.ssh_credentials())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH setup timed out"))?
            .map_err(|error| io::Error::other(error.message))?;
    let files = SessionFiles::create(
        Path::new(SESSION_DIRECTORY),
        &admission.public_key,
        host_private_key
            .to_str()
            .ok_or_else(|| io::Error::other("invalid host key path"))?,
    )?;
    let mut child = tokio::process::Command::new("/usr/sbin/sshd")
        .args(["-i", "-e", "-f"])
        .arg(files.config())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        write_frame(
            &mut stream,
            &SshReady {
                public_key: host_public_key,
            },
        ),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH readiness timed out"))?
    .map_err(io::Error::other)?;

    let (mut client_read, mut client_write) = tokio::io::split(stream);
    let mut sshd_in = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("sshd stdin unavailable"))?;
    let mut sshd_out = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("sshd stdout unavailable"))?;
    // Drain sshd output rather than racing child.wait(): the SSH exit-status
    // packet can still be buffered after sshd exits. Client EOF closes the
    // session immediately so a disconnected client cannot retain its permit.
    tokio::select! {
        result = tokio::io::copy(&mut client_read, &mut sshd_in) => { result?; }
        result = tokio::io::copy(&mut sshd_out, &mut client_write) => { result?; }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    drop(files);
    Ok(())
}
