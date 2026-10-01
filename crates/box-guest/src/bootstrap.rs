//! Pre-systemd template barrier. No workload execution is available here.
use crate::{Agent, Initializer, SystemInitializer};
use box_protocol::{
    ErrorCode, HOST_CID, ProtocolError, Request, Response, VSOCK_PORT, read_frame, write_frame,
};
use std::{io, io::Write, os::unix::fs::OpenOptionsExt, path::Path, sync::Arc, time::Duration};
use tokio_vsock::{VsockAddr, VsockListener};

pub const MARKER: &str = "/run/boxd-initialized";

pub async fn handle<I: Initializer>(agent: &Agent<I>, request: Request, marker: &Path) -> Response {
    match request {
        Request::Exec(_) => Response::Error(ProtocolError::new(
            ErrorCode::NotInitialized,
            "systemd initialization has not completed",
        )),
        Request::Hello { .. } => {
            let mut response = agent.handle(request).await;
            if let Response::Hello { initialized, .. } = &mut response {
                // Only the systemd-managed agent can advertise workload readiness,
                // including when initialization succeeded but marker creation failed.
                *initialized = false;
            }
            response
        }
        Request::Initialize(ref identity) => {
            let machine_id = identity.machine_id.clone();
            let response = agent.handle(request).await;
            if matches!(response, Response::Initialized { .. }) {
                let result = (|| -> io::Result<()> {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(marker)?;
                    writeln!(file, "{machine_id}")?;
                    file.sync_all()
                })();
                if let Err(error) = result {
                    return Response::Error(ProtocolError::new(
                        ErrorCode::InitializationFailed,
                        error.to_string(),
                    ));
                }
            }
            response
        }
    }
}

pub async fn serve() -> io::Result<()> {
    serve_with(Arc::new(SystemInitializer)).await
}

pub async fn serve_with<I: Initializer>(initializer: Arc<I>) -> io::Result<()> {
    let listener = VsockListener::bind(VsockAddr::new(libc::VMADDR_CID_ANY, VSOCK_PORT))?;
    let agent = Agent::new(initializer, 1);
    loop {
        let (mut stream, peer) = listener.accept().await?;
        if peer.cid() != HOST_CID {
            continue;
        }
        let Ok(Ok(request)) = tokio::time::timeout(
            Duration::from_secs(5),
            read_frame::<_, Request>(&mut stream),
        )
        .await
        else {
            continue;
        };
        let response = handle(&agent, request, Path::new(MARKER)).await;
        if matches!(response, Response::Initialized { .. }) {
            // Close the listener before acknowledging: the next Hello can only
            // come from the systemd service, never this bootstrap process.
            drop(listener);
            tokio::time::timeout(Duration::from_secs(5), write_frame(&mut stream, &response))
                .await
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let _ =
            tokio::time::timeout(Duration::from_secs(5), write_frame(&mut stream, &response)).await;
    }
}

pub fn mount_filesystems() -> io::Result<()> {
    use std::ffi::CString;
    for (target, kind) in [
        ("/proc", "proc"),
        ("/sys", "sysfs"),
        ("/dev", "devtmpfs"),
        ("/run", "tmpfs"),
    ] {
        std::fs::create_dir_all(target)?;
        let target = CString::new(target).unwrap();
        let kind = CString::new(kind).unwrap();
        // SAFETY: strings are NUL terminated; no mount-specific data is passed.
        if unsafe {
            libc::mount(
                kind.as_ptr(),
                target.as_ptr(),
                kind.as_ptr(),
                libc::MS_NOSUID,
                std::ptr::null(),
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EBUSY) {
                return Err(error);
            }
        }
    }
    // Materialize the prepared kernel's boot ID before snapshotting, just as
    // the fixture does. It is not a per-clone machine identity.
    std::fs::write(
        "/run/prepared-boot-id",
        std::fs::read("/proc/sys/kernel/random/boot_id")?,
    )
}
