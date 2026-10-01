use std::{io, os::unix::process::CommandExt, process::Command};

fn main() -> io::Result<()> {
    if std::process::id() != 1 {
        return Err(io::Error::other("box-init must run as guest PID 1"));
    }
    box_guest::bootstrap::mount_filesystems()?;
    // Only pristine images enter warm preparation. A user's existing box may
    // contain services and credentials: cold restart provisions before systemd,
    // never attempts to turn that state back into a shareable template.
    let prepare = box_guest::warm::preparation_requested(
        &std::fs::read_to_string("/proc/cmdline")?,
        &std::fs::read_to_string("/etc/machine-id")?,
    );
    // No runtime threads or vsock descriptors survive the exec into systemd.
    if !prepare {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(box_guest::bootstrap::serve())?;
    }
    Err(Command::new("/sbin/init").exec())
}
