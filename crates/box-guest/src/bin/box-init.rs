use std::{io, os::unix::process::CommandExt, process::Command};

fn main() -> io::Result<()> {
    if std::process::id() != 1 {
        return Err(io::Error::other("box-init must run as guest PID 1"));
    }
    box_guest::bootstrap::mount_filesystems()?;
    // No runtime threads or vsock descriptors survive the exec into systemd.
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(box_guest::bootstrap::serve())?;
    }
    Err(Command::new("/sbin/init").exec())
}
