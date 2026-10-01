use box_guest::{Agent, SystemInitializer};
use box_protocol::{
    ErrorCode, HOST_CID, ProtocolError, Request, Response, VSOCK_PORT, read_frame, write_frame,
};
use std::{io, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;
use tokio_vsock::{VsockAddr, VsockListener};

#[tokio::main]
async fn main() -> io::Result<()> {
    let agent = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => Agent::new(Arc::new(SystemInitializer), 8),
        [flag] if flag == "--warm-bootstrap" => return box_guest::warm::serve().await,
        [flag] if flag == "--systemd-service" => Agent::from_boot_marker(
            Arc::new(SystemInitializer),
            8,
            std::path::Path::new(box_guest::bootstrap::MARKER),
        )?,
        _ => {
            return Err(io::Error::other(
                "usage: box-guest [--systemd-service | --warm-bootstrap]",
            ));
        }
    };
    let listener = VsockListener::bind(VsockAddr::new(libc::VMADDR_CID_ANY, VSOCK_PORT))?;
    let agent = Arc::new(agent);
    let connections = Arc::new(tokio::sync::Semaphore::new(32));
    loop {
        let (stream, peer) = listener.accept().await?;
        if peer.cid() != HOST_CID {
            continue;
        }
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            continue;
        };
        let agent = agent.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let (mut reader, mut writer) = tokio::io::split(stream);
            let request = match tokio::time::timeout(
                Duration::from_secs(5),
                read_frame::<_, Request>(&mut reader),
            )
            .await
            {
                Ok(Ok(request)) => request,
                Ok(Err(box_protocol::FrameError::Json(err))) => {
                    let response = Response::Error(ProtocolError::new(
                        ErrorCode::InvalidRequest,
                        err.to_string(),
                    ));
                    let _ = tokio::time::timeout(
                        Duration::from_secs(5),
                        write_frame(&mut writer, &response),
                    )
                    .await;
                    return;
                }
                _ => return,
            };
            let (cancel, cancelled) = tokio::sync::oneshot::channel();
            let monitor = tokio::spawn(async move {
                let mut byte = [0_u8; 1];
                let _ = reader.read(&mut byte).await;
                let _ = cancel.send(());
            });
            let response = agent.handle_with_cancellation(request, cancelled).await;
            let _ =
                tokio::time::timeout(Duration::from_secs(5), write_frame(&mut writer, &response))
                    .await;
            monitor.abort();
            let _ = monitor.await;
        });
    }
}
