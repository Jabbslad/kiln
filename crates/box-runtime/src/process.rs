use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

impl FileIdentity {
    pub fn read(path: &Path) -> Result<Self> {
        // Follow procfs magic links to the object in the target mount namespace.
        // readlink text is diagnostic only: pivot_root can render it as "/".
        let metadata = fs::metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub start_ticks: u64,
    pub executable: PathBuf,
    pub directory: PathBuf,
    #[serde(default)]
    pub executable_object: Option<FileIdentity>,
    #[serde(default)]
    pub directory_object: Option<FileIdentity>,
}

impl Identity {
    pub fn matches_paths(&self, directory: &Path, executables: &[&Path]) -> Result<bool> {
        if self.directory_object.as_ref() != Some(&FileIdentity::read(directory)?) {
            return Ok(false);
        }
        for executable in executables {
            match FileIdentity::read(executable) {
                Ok(object) if self.executable_object.as_ref() == Some(&object) => return Ok(true),
                Ok(_) => (),
                // The jail copy does not exist yet while jailer is starting.
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }
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
        executable_object: Some(FileIdentity::read(&root.join("exe"))?),
        directory_object: Some(FileIdentity::read(&root.join("cwd"))?),
    })
}

pub fn find(directory: &Path, executables: &[&Path]) -> Result<Option<Identity>> {
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
            && identity.matches_paths(directory, executables)?
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

pub fn same_launch(expected: &Identity, actual: &Identity, successor: Option<&Path>) -> bool {
    expected.pid == actual.pid
        && expected.start_ticks == actual.start_ticks
        && expected.directory_object.as_ref().map_or_else(
            || expected.directory == actual.directory, // pre-isolation development records
            |object| actual.directory_object.as_ref() == Some(object),
        )
        && (expected.executable_object.as_ref().map_or_else(
            || expected.executable == actual.executable,
            |object| actual.executable_object.as_ref() == Some(object),
        ) || successor
            .and_then(|path| FileIdentity::read(path).ok())
            .is_some_and(|object| actual.executable_object.as_ref() == Some(&object)))
}

pub fn terminate(identity: &Identity) -> Result<()> {
    terminate_with_successor(identity, None)
}

pub fn terminate_with_successor(identity: &Identity, successor: Option<&Path>) -> Result<()> {
    // Pin the process before checking identity; signaling by pidfd cannot hit
    // a different process if the numeric PID is subsequently reused.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: pidfd_open returned a fresh owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    if !same_launch(identity, &identify(identity.pid)?, successor) {
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
    fn namespace_path_changes_do_not_change_file_identity() {
        let expected = identify(std::process::id()).unwrap();
        let mut actual = expected.clone();
        actual.directory = "/".into();
        actual.executable = "/firecracker".into();
        assert!(same_launch(&expected, &actual, None));
        let mut replaced = actual.clone();
        replaced.directory_object.as_mut().unwrap().inode += 1;
        assert!(!same_launch(&expected, &replaced, None));
        replaced = actual;
        replaced.executable_object.as_mut().unwrap().inode += 1;
        assert!(!same_launch(&expected, &replaced, None));
    }

    #[test]
    fn discovery_accepts_directory_alias_but_not_another_inode() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("work");
        fs::create_dir(&directory).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&directory, &alias).unwrap();
        let executable = fs::canonicalize("/bin/sleep").unwrap();
        let mut child = std::process::Command::new(&executable)
            .arg("60")
            .current_dir(&directory)
            .spawn()
            .unwrap();
        let found = find(&alias, &[&executable]).unwrap();
        let other = find(root.path(), &[&executable]).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found.unwrap().pid, child.id());
        assert!(other.is_none());
    }

    #[test]
    fn permits_only_the_owned_launchers_expected_exec_transition() {
        use std::io::Write;
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "read -r line; exec /bin/sleep 60"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let launcher = identify(child.id()).unwrap();
        child.stdin.take().unwrap().write_all(b"go\n").unwrap();
        let executable = fs::canonicalize("/bin/sleep").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let actual = loop {
            let actual = identify(child.id()).unwrap();
            if actual.executable == executable {
                break actual;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert!(same_launch(&launcher, &actual, Some(&executable)));
        assert!(!same_launch(&launcher, &actual, None));
        assert!(!same_launch(
            &launcher,
            &actual,
            Some(Path::new("/unexpected"))
        ));
        let mut reused = launcher.clone();
        reused.start_ticks += 1;
        assert!(terminate_with_successor(&reused, Some(&executable)).is_err());
        assert!(child.try_wait().unwrap().is_none());
        terminate_with_successor(&launcher, Some(&executable)).unwrap();
        child.wait().unwrap();
    }

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
