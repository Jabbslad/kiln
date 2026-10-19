use kiln_guest::{Agent, SystemInitializer};
use kiln_protocol::{
    ErrorCode, HOST_CID, ProtocolError, Request, Response, SSH_VSOCK_PORT, VSOCK_PORT, read_frame,
    write_frame,
};
use std::{future::Future, io, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;
use tokio_vsock::{VsockAddr, VsockListener};

#[tokio::main]
async fn main() -> io::Result<()> {
    let agent = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => Agent::new(Arc::new(SystemInitializer), 8),
        [flag] if flag == "--warm-bootstrap" => return kiln_guest::warm::serve().await,
        [flag] if flag == "--systemd-service" => Agent::from_boot_marker(
            Arc::new(SystemInitializer),
            8,
            std::path::Path::new(kiln_guest::bootstrap::MARKER),
        )?,
        _ => {
            return Err(io::Error::other(
                "usage: kiln-guest [--systemd-service | --warm-bootstrap]",
            ));
        }
    };
    let listener = VsockListener::bind(VsockAddr::new(libc::VMADDR_CID_ANY, VSOCK_PORT))?;
    let agent = Arc::new(agent);
    let ssh_listener = VsockListener::bind(VsockAddr::new(libc::VMADDR_CID_ANY, SSH_VSOCK_PORT))?;
    let ssh_agent = agent.clone();
    tokio::spawn(async move {
        let sessions = Arc::new(tokio::sync::Semaphore::new(8));
        run_accept_loop("ssh", &ssh_listener, |(stream, peer)| {
            let sessions = sessions.clone();
            let agent = ssh_agent.clone();
            async move {
                if peer.cid() != HOST_CID {
                    return;
                }
                let Ok(permit) = sessions.try_acquire_owned() else {
                    return;
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = kiln_guest::ssh::serve_session(stream, &agent).await;
                });
            }
        })
        .await;
    });
    let connections = Arc::new(tokio::sync::Semaphore::new(32));
    run_accept_loop("exec", &listener, |(stream, peer)| {
        let connections = connections.clone();
        let agent = agent.clone();
        async move {
            if peer.cid() != HOST_CID {
                return;
            }
            let Ok(permit) = connections.try_acquire_owned() else {
                return;
            };
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
                    Ok(Err(kiln_protocol::FrameError::Json(err))) => {
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
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    write_frame(&mut writer, &response),
                )
                .await;
                monitor.abort();
                let _ = monitor.await;
            });
        }
    })
    .await;
    Ok(())
}

trait Incoming {
    type Item;
    fn accept_next(&self) -> impl Future<Output = io::Result<Self::Item>> + Send;
}

impl Incoming for tokio_vsock::VsockListener {
    type Item = (tokio_vsock::VsockStream, tokio_vsock::VsockAddr);

    fn accept_next(&self) -> impl Future<Output = io::Result<Self::Item>> + Send {
        tokio_vsock::VsockListener::accept(self)
    }
}

async fn run_accept_loop<L, H, HFut>(label: &str, listener: &L, mut handle: H)
where
    L: Incoming,
    H: FnMut(L::Item) -> HFut,
    HFut: Future<Output = ()>,
{
    loop {
        match listener.accept_next().await {
            Ok(item) => handle(item).await,
            Err(error) => {
                eprintln!("kiln-guest: {label} accept error: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    struct Scripted {
        items: Mutex<VecDeque<io::Result<i32>>>,
    }

    impl Incoming for Scripted {
        type Item = i32;

        fn accept_next(&self) -> impl Future<Output = io::Result<Self::Item>> + Send {
            let next = self.items.lock().unwrap().pop_front();
            async move {
                match next {
                    Some(result) => result,
                    None => std::future::pending().await,
                }
            }
        }
    }

    #[tokio::test]
    async fn accept_loop_keeps_running_after_accept_errors() {
        let scripted = Scripted {
            items: Mutex::new(VecDeque::from([
                Err(io::Error::other("temporary")),
                Ok(1),
                Err(io::Error::other("again")),
                Ok(2),
            ])),
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_task = seen.clone();
        let task = tokio::spawn(async move {
            run_accept_loop("ssh", &scripted, |item| {
                let seen_task = seen_task.clone();
                async move {
                    seen_task.lock().unwrap().push(item);
                }
            })
            .await;
        });
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if seen.lock().unwrap().as_slice() == [1, 2] {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        task.abort();
        result.expect("accept loop stopped after an accept error");
    }
}
