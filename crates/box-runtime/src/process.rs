use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub start_ticks: u64,
    pub executable: PathBuf,
    pub directory: PathBuf,
}

pub fn identify(pid: u32) -> Result<Identity> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let stat = fs::read_to_string(root.join("stat"))?;
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| Error::Invalid("invalid proc stat".into()))?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return Err(Error::Invalid("process is a zombie".into()));
    }
    let start_ticks = fields
        .get(19)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Invalid("missing process start ticks".into()))?;
    Ok(Identity {
        pid,
        start_ticks,
        executable: fs::read_link(root.join("exe"))?,
        directory: fs::read_link(root.join("cwd"))?,
    })
}

pub fn find(directory: &Path, executable: &Path) -> Result<Option<Identity>> {
    let mut matches = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Ok(identity) = identify(pid)
            && identity.directory == directory
            && identity.executable == executable
        {
            matches.push(identity);
        }
    }
    if matches.len() > 1 {
        return Err(Error::Invalid(
            "multiple VMM processes own an instance directory; refusing recovery".into(),
        ));
    }
    Ok(matches.pop())
}

pub fn terminate(identity: &Identity) -> Result<()> {
    // Pin the process before checking identity; signaling by pidfd cannot hit
    // a different process if the numeric PID is subsequently reused.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: pidfd_open returned a fresh owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    if identify(identity.pid)? != *identity {
        return Err(Error::Invalid(
            "process identity changed; refusing to signal".into(),
        ));
    }
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            descriptor.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut poll = libc::pollfd {
        fd: descriptor.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll references one valid descriptor and initialized pollfd.
    if unsafe { libc::poll(&mut poll, 1, 5000) } <= 0 {
        return Err(Error::Invalid("VMM termination was not confirmed".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_stale_identity_and_reaps_owned_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let actual = identify(child.id());
        if actual.is_err() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        let identity = actual.unwrap();
        let mut stale = identity.clone();
        stale.start_ticks += 1;
        assert!(terminate(&stale).is_err());
        assert!(child.try_wait().unwrap().is_none());
        terminate(&identity).unwrap();
        child.wait().unwrap();
        assert!(identify(identity.pid).is_err());
    }
}
