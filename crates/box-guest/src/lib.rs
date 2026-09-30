use box_protocol::{
    ErrorCode, ExecRequest, ExecResult, InitializeRequest, MAX_OUTPUT_SIZE, PROTOCOL_VERSION,
    ProtocolError, Request, Response,
};
use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::Semaphore,
};

pub trait Initializer: Send + Sync + 'static {
    fn initialize(&self, request: &InitializeRequest) -> Result<(), String>;
}

#[derive(Default)]
pub struct SystemInitializer;

impl Initializer for SystemInitializer {
    fn initialize(&self, request: &InitializeRequest) -> Result<(), String> {
        validate_identity(request)?;
        write_all(Path::new("/dev/urandom"), &request.entropy)?;
        let hostname = std::ffi::CString::new(request.hostname.as_str())
            .map_err(|_| "hostname contains NUL".to_string())?;
        // SAFETY: the CString remains valid for the duration of sethostname.
        if unsafe { libc::sethostname(hostname.as_ptr(), request.hostname.len()) } != 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        write_all(
            Path::new("/etc/machine-id"),
            format!("{}\n", request.machine_id).as_bytes(),
        )
    }
}

fn write_all(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(path != Path::new("/dev/urandom"))
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(data)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn validate_identity(request: &InitializeRequest) -> Result<(), String> {
    if request.hostname.is_empty()
        || request.hostname.len() > 63
        || !request
            .hostname
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err("hostname must be 1..63 ASCII hostname characters".into());
    }
    if request.machine_id.len() != 32 || !request.machine_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("machine_id must be 32 hexadecimal characters".into());
    }
    if request.entropy.len() < 32 {
        return Err("at least 32 bytes of host entropy are required".into());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Barrier {
    Uninitialized,
    Initializing,
    Initialized,
    Failed,
}

pub struct Agent<I: Initializer> {
    initializer: Arc<I>,
    barrier: Mutex<Barrier>,
    executions: Arc<Semaphore>,
}

impl<I: Initializer> Agent<I> {
    pub fn new(initializer: Arc<I>, max_concurrency: usize) -> Self {
        Self {
            initializer,
            barrier: Mutex::new(Barrier::Uninitialized),
            executions: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }

    pub async fn handle(&self, request: Request) -> Response {
        let (_sender, never_cancelled) = tokio::sync::oneshot::channel();
        self.handle_with_cancellation(request, never_cancelled)
            .await
    }

    pub async fn handle_with_cancellation(
        &self,
        request: Request,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) -> Response {
        match request {
            Request::Hello { version } => {
                if version != PROTOCOL_VERSION {
                    error(
                        ErrorCode::UnsupportedVersion,
                        "unsupported protocol version",
                    )
                } else {
                    Response::Hello {
                        version: PROTOCOL_VERSION,
                        initialized: self.is_initialized(),
                    }
                }
            }
            Request::Initialize(request) => self.initialize(request),
            Request::Exec(request) => self.exec(request, cancelled).await,
        }
    }

    fn is_initialized(&self) -> bool {
        *self.barrier.lock().expect("barrier poisoned") == Barrier::Initialized
    }

    fn initialize(&self, request: InitializeRequest) -> Response {
        {
            let mut state = self.barrier.lock().expect("barrier poisoned");
            if *state != Barrier::Uninitialized {
                return error(
                    ErrorCode::AlreadyInitialized,
                    "initialization was already attempted",
                );
            }
            *state = Barrier::Initializing;
        }
        match self.initializer.initialize(&request) {
            Ok(()) => {
                *self.barrier.lock().expect("barrier poisoned") = Barrier::Initialized;
                Response::Initialized {
                    version: PROTOCOL_VERSION,
                }
            }
            Err(message) => {
                *self.barrier.lock().expect("barrier poisoned") = Barrier::Failed;
                error(ErrorCode::InitializationFailed, message)
            }
        }
    }

    async fn exec(
        &self,
        request: ExecRequest,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) -> Response {
        if !self.is_initialized() {
            return error(
                ErrorCode::NotInitialized,
                "guest initialization is required",
            );
        }
        if request.argv.is_empty() || request.argv[0].is_empty() || request.timeout_ms == 0 {
            return error(
                ErrorCode::InvalidRequest,
                "argv and a non-zero timeout are required",
            );
        }
        let Ok(_permit) = self.executions.clone().try_acquire_owned() else {
            return error(ErrorCode::Busy, "execution concurrency limit reached");
        };
        match execute(request, cancelled).await {
            Ok(result) => Response::Exec(result),
            Err(message) => error(ErrorCode::ExecutionFailed, message),
        }
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::Error(ProtocolError::new(code, message))
}

async fn drain_capped<R: AsyncRead + Unpin>(mut reader: R) -> io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut truncated = false;
    let mut chunk = [0; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let remaining = MAX_OUTPUT_SIZE.saturating_sub(output.len());
        output.extend_from_slice(&chunk[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    Ok((output, truncated))
}

async fn execute(
    request: ExecRequest,
    mut cancelled: tokio::sync::oneshot::Receiver<()>,
) -> Result<ExecResult, String> {
    let mut command = Command::new(&request.argv[0]);
    command
        .args(&request.argv[1..])
        .envs(request.env)
        .kill_on_drop(true);
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // SAFETY: setsid is async-signal-safe and does not access parent memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id().ok_or_else(|| "child has no pid".to_string())? as i32;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr unavailable".to_string())?;
    // Keep the deadline over both process exit AND pipe draining. A background
    // descendant may retain a pipe after its parent has exited.
    let (completed, disconnected) = {
        let execution =
            async { tokio::join!(child.wait(), drain_capped(stdout), drain_capped(stderr)) };
        tokio::select! {
            result = execution => (Some(result), false),
            _ = tokio::time::sleep(Duration::from_millis(request.timeout_ms)) => (None, false),
            _ = &mut cancelled => (None, true),
        }
    };
    let Some((status, stdout, stderr)) = completed else {
        // SAFETY: the child established its own session/process group. The
        // group remains reserved while any of its descendants exist.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
        child.wait().await.map_err(|e| e.to_string())?;
        if disconnected {
            return Err("host disconnected; execution was cancelled".into());
        }
        return Ok(ExecResult {
            stdout: vec![],
            stderr: vec![],
            exit_code: None,
            truncated: true,
            timed_out: true,
        });
    };
    let status = status.map_err(|e| e.to_string())?;
    let (stdout, stdout_truncated) = stdout.map_err(|e| e.to_string())?;
    let (stderr, stderr_truncated) = stderr.map_err(|e| e.to_string())?;
    Ok(ExecResult {
        stdout,
        stderr,
        exit_code: status.code(),
        truncated: stdout_truncated || stderr_truncated,
        timed_out: false,
    })
}

#[derive(Default)]
pub struct FakeInitializer {
    calls: std::sync::atomic::AtomicUsize,
}

impl FakeInitializer {
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Initializer for FakeInitializer {
    fn initialize(&self, request: &InitializeRequest) -> Result<(), String> {
        validate_identity(request)?;
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

impl Agent<FakeInitializer> {
    pub fn initialized_for_test(max_concurrency: usize) -> Self {
        Self {
            initializer: Arc::new(FakeInitializer::default()),
            barrier: Mutex::new(Barrier::Initialized),
            executions: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }
}
