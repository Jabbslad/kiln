//! Dashboard shell session. This is not a journal operation: the browser
//! speaks a same-origin WebSocket, and this process opens the box through the
//! existing SSH upgrade, then runs OpenSSH on a PTY the way `kiln ssh` does.
//! The key exists only for that one admission.

use crate::Failure;
use axum::{
    extract::ws::{Message, WebSocket},
    http::{HeaderValue, StatusCode, header},
};
use futures_util::{SinkExt, StreamExt};
use kiln_api::{SshReady, valid_ssh_public_key};
use std::{
    ffi::CString,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    time::{sleep, timeout},
};

pub const IDLE: Duration = Duration::from_secs(15 * 60);
pub const MAX_SESSION: Duration = Duration::from_secs(2 * 60 * 60);
const OPEN_LIMIT: Duration = Duration::from_secs(20);
const FRAME_LIMIT: usize = 32 * 1024;

pub struct Session {
    master: tokio::io::unix::AsyncFd<Pty>,
    pid: libc::pid_t,
    _dir: tempfile::TempDir,
    splice: tokio::task::JoinHandle<()>,
}

struct Pty(OwnedFd);

impl AsRawFd for Pty {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.splice.abort();
        stop_child(self.pid);
    }
}

pub fn tunnel_client(ca_pem: &[u8]) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .pool_max_idle_per_host(0)
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem)?)
        .tls_built_in_root_certs(false)
        .build()?)
}

pub async fn open(
    client: &reqwest::Client,
    upstream: &reqwest::Url,
    authorization: &HeaderValue,
    box_id: &str,
    host_key: &str,
) -> Result<Session, Failure> {
    if !kiln_api::valid_id(box_id) || !valid_ssh_public_key(host_key) {
        return Err(upstream_failure());
    }
    let dir = tempfile::TempDir::new().map_err(|_| shell_failure())?;
    let key = dir.path().join("id_ed25519");
    let public = ssh_keygen(&key)?;
    let upgraded = ssh_upgrade(client, upstream, authorization, box_id, &public).await?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| shell_failure())?;
    let port = listener
        .local_addr()
        .map_err(|_| shell_failure())?
        .port()
        .to_string();
    let known = dir.path().join("known_hosts");
    std::fs::write(&known, format!("kiln-{box_id} {host_key}\n")).map_err(|_| shell_failure())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&known, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| shell_failure())?;
    }
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let splice = tokio::spawn(async move {
        let accepted = timeout(OPEN_LIMIT, listener.accept()).await;
        let _ = accepted_tx.send(accepted.is_ok());
        if let Ok(Ok((mut stream, _))) = accepted {
            let mut upgraded = upgraded;
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut upgraded).await;
        }
    });
    let alias = format!("kiln-{box_id}");
    let known_display = path_arg(&known)?;
    let key_display = path_arg(&key)?;
    let args = [
        "ssh".into(),
        "-tt".into(),
        "-F".into(),
        "/dev/null".into(),
        "-i".into(),
        key_display,
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        format!("UserKnownHostsFile={known_display}"),
        "-o".into(),
        "GlobalKnownHostsFile=/dev/null".into(),
        "-o".into(),
        format!("HostKeyAlias={alias}"),
        "-o".into(),
        "ForwardAgent=no".into(),
        "-o".into(),
        "NumberOfPasswordPrompts=0".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "AddressFamily=inet".into(),
        "-o".into(),
        "UpdateHostKeys=no".into(),
        "-o".into(),
        "LogLevel=ERROR".into(),
        "-p".into(),
        port,
        "root@127.0.0.1".into(),
    ];
    let (master, pid) = match spawn_ssh(&args) {
        Ok(started) => started,
        Err(error) => {
            splice.abort();
            return Err(error);
        }
    };
    if resize(master.as_raw_fd(), pid, 24, 80).is_err() {
        splice.abort();
        stop_child(pid);
        return Err(shell_failure());
    }
    let master = match tokio::io::unix::AsyncFd::new(master) {
        Ok(master) => master,
        Err(_) => {
            splice.abort();
            stop_child(pid);
            return Err(shell_failure());
        }
    };
    match timeout(OPEN_LIMIT, accepted_rx).await {
        Ok(Ok(true)) => Ok(Session {
            master,
            pid,
            _dir: dir,
            splice,
        }),
        _ => {
            splice.abort();
            stop_child(pid);
            Err(shell_failure())
        }
    }
}

fn stop_child(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
}

pub async fn bridge(mut socket: WebSocket, session: Session, idle: Duration, max: Duration) {
    let started = Instant::now();
    let mut last = Instant::now();
    let mut reason = "eof";
    loop {
        let remaining = idle
            .saturating_sub(last.elapsed())
            .min(max.saturating_sub(started.elapsed()));
        if remaining.is_zero() {
            reason = if started.elapsed() >= max {
                "max"
            } else {
                "idle"
            };
            break;
        }
        tokio::select! {
            message = socket.next() => {
                last = Instant::now();
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if !apply_client(&session, text.as_str()).await {
                            reason = "closed";
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if bytes.len() > FRAME_LIMIT || write_pty(&session.master, &bytes).await.is_err() {
                            reason = "closed";
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                }
            }
            read = read_pty(&session.master) => {
                last = Instant::now();
                match read {
                    Ok(bytes) if bytes.is_empty() => break,
                    Ok(bytes) => {
                        if socket.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            () = sleep(remaining) => {
                reason = if started.elapsed() >= max { "max" } else { "idle" };
                break;
            }
        }
    }
    let _ = socket
        .send(Message::Text(
            format!(r#"{{"type":"closed","reason":"{reason}"}}"#).into(),
        ))
        .await;
    let _ = socket.close().await;
}

async fn apply_client(session: &Session, text: &str) -> bool {
    if text.len() > FRAME_LIMIT {
        return false;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
        && value.get("type").and_then(|item| item.as_str()) == Some("resize")
    {
        let cols = value
            .get("cols")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        let rows = value
            .get("rows")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        return (1..=512).contains(&cols)
            && (1..=256).contains(&rows)
            && resize(
                session.master.as_raw_fd(),
                session.pid,
                rows as u16,
                cols as u16,
            )
            .is_ok();
    }
    write_pty(&session.master, text.as_bytes()).await.is_ok()
}

async fn ssh_upgrade(
    client: &reqwest::Client,
    upstream: &reqwest::Url,
    authorization: &HeaderValue,
    box_id: &str,
    public_key: &str,
) -> Result<reqwest::Upgraded, Failure> {
    let mut url = upstream.clone();
    url.set_path(&format!("/v1/boxes/{box_id}/ssh"));
    let response = client
        .get(url)
        .header(header::AUTHORIZATION, authorization)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
        .header(kiln_api::SSH_KEY_HEADER, public_key)
        .send()
        .await
        .map_err(|_| upstream_failure())?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(upstream_status(response).await);
    }
    if response
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        != Some(kiln_api::SSH_UPGRADE)
    {
        return Err(upstream_failure());
    }
    response.upgrade().await.map_err(|_| upstream_failure())
}

async fn upstream_status(response: reqwest::Response) -> Failure {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = response.bytes().await.unwrap_or_default();
    if let Ok(error) = serde_json::from_slice::<kiln_api::ApiError>(&bytes)
        && error.code.len() <= 64
        && error.message.len() <= 400
        && !error.message.contains('/')
    {
        return Failure::new(status, &error.code, &error.message);
    }
    upstream_failure()
}

fn ssh_keygen(path: &Path) -> Result<String, Failure> {
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
        .arg(path)
        .status()
        .map_err(|_| client_failure())?;
    if !status.success() {
        return Err(client_failure());
    }
    let public =
        std::fs::read_to_string(path.with_extension("pub")).map_err(|_| client_failure())?;
    let public = public.trim();
    if !valid_ssh_public_key(public) {
        return Err(client_failure());
    }
    Ok(public.to_owned())
}

fn path_arg(path: &Path) -> Result<String, Failure> {
    let text = path
        .to_str()
        .filter(|text| !text.is_empty() && !text.contains([' ', '\n', '\r', '"', '\'']))
        .ok_or_else(shell_failure)?;
    Ok(text.to_owned())
}

fn spawn_ssh(args: &[String]) -> Result<(Pty, libc::pid_t), Failure> {
    let c_args: Vec<CString> = args
        .iter()
        .map(|arg| CString::new(arg.as_str()).map_err(|_| shell_failure()))
        .collect::<Result<_, _>>()?;
    let mut pointers: Vec<*const libc::c_char> = c_args.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(std::ptr::null());
    let mut master: libc::c_int = -1;
    // SAFETY: the child calls execvp or _exit before any other Rust code, so it
    // does not take a lock held by another thread.
    let pid = unsafe {
        libc::forkpty(
            &mut master,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if pid < 0 {
        return Err(shell_failure());
    }
    if pid == 0 {
        unsafe {
            libc::execvp(pointers[0], pointers.as_ptr());
            libc::_exit(127);
        }
    }
    let owned = unsafe { OwnedFd::from_raw_fd(master) };
    let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        unsafe { libc::kill(pid, libc::SIGKILL) };
        return Err(shell_failure());
    }
    Ok((Pty(owned), pid))
}

fn resize(master: RawFd, pid: libc::pid_t, rows: u16, columns: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::kill(pid, libc::SIGWINCH) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

async fn read_pty(master: &tokio::io::unix::AsyncFd<Pty>) -> io::Result<Vec<u8>> {
    loop {
        let mut guard = master.readable().await?;
        let mut buffer = [0u8; 8192];
        let read = guard.try_io(|pty| {
            let count =
                unsafe { libc::read(pty.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if count < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(buffer[..count as usize].to_vec())
            }
        });
        match read {
            Ok(result) => return result,
            Err(_blocked) => continue,
        }
    }
}

async fn write_pty(master: &tokio::io::unix::AsyncFd<Pty>, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let mut guard = master.writable().await?;
        let wrote = guard.try_io(|pty| {
            let count = unsafe {
                libc::write(
                    pty.as_raw_fd(),
                    bytes[offset..].as_ptr().cast(),
                    bytes.len() - offset,
                )
            };
            if count < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(count as usize)
            }
        });
        match wrote {
            Ok(Ok(0)) => return Err(io::Error::new(io::ErrorKind::WriteZero, "pty write")),
            Ok(Ok(count)) => offset += count,
            Ok(Err(error)) => return Err(error),
            Err(_blocked) => continue,
        }
    }
    Ok(())
}

fn client_failure() -> Failure {
    Failure::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "terminal_unavailable",
        "The host shell client is unavailable.",
    )
}

fn shell_failure() -> Failure {
    Failure::new(
        StatusCode::BAD_GATEWAY,
        "upstream",
        "The shell did not start.",
    )
}

fn upstream_failure() -> Failure {
    Failure::new(
        StatusCode::BAD_GATEWAY,
        "upstream",
        "The local API did not answer.",
    )
}

pub fn host_key(bytes: &[u8]) -> Result<String, Failure> {
    let ready: SshReady = serde_json::from_slice(bytes).map_err(|_| upstream_failure())?;
    if !valid_ssh_public_key(&ready.public_key) {
        return Err(upstream_failure());
    }
    Ok(ready.public_key)
}
