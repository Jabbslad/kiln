use crate::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const FICLONE: libc::c_ulong = 0x4004_9409;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum CopyMethod {
    Reflink,
    Copy,
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn private_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(Error::Invalid(format!(
                    "{} is not a directory",
                    path.display()
                )));
            }
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(Error::Invalid(format!(
                    "{} is not owned by the current user",
                    path.display()
                )));
            }
            if metadata.mode() & 0o077 != 0 {
                return Err(Error::Invalid(format!(
                    "{} is accessible by group or other users",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            File::open(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .sync_all()?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid(format!("{} has no parent", path.display())))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid(format!("{} has no file name", path.display())))?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    Ok(serde_json::from_reader(file)?)
}

pub fn copy_disk(src: &Path, dst: &Path, deadline: Instant) -> Result<CopyMethod> {
    if Instant::now() >= deadline {
        return Err(Error::Invalid("disk copy deadline expired".into()));
    }
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(src)?;
    let source_metadata = source.metadata()?;
    if !source_metadata.file_type().is_file() || source_metadata.file_type().is_block_device() {
        return Err(Error::Invalid(format!(
            "{} is not a regular file",
            src.display()
        )));
    }
    let mut destination = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dst)?;

    let result = (|| -> Result<CopyMethod> {
        if unsafe { libc::ioctl(destination.as_raw_fd(), FICLONE, source.as_raw_fd()) } == 0 {
            if Instant::now() >= deadline {
                return Err(Error::Invalid("disk copy deadline expired".into()));
            }
            destination.sync_all()?;
            return Ok(CopyMethod::Reflink);
        }
        let reflink_error = std::io::Error::last_os_error();
        if !matches!(
            reflink_error.raw_os_error(),
            Some(libc::EOPNOTSUPP | libc::ENOTTY | libc::EINVAL | libc::EXDEV | libc::ENOSYS)
        ) {
            return Err(reflink_error.into());
        }

        ensure_free_space(&destination, source_metadata.len())?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Invalid("disk copy deadline expired".into()));
            }
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if buffer[..count].iter().all(|byte| *byte == 0) {
                destination.seek(SeekFrom::Current(count as i64))?;
            } else {
                destination.write_all(&buffer[..count])?;
            }
        }
        destination.set_len(source_metadata.len())?;
        destination.sync_all()?;
        Ok(CopyMethod::Copy)
    })();
    if result.is_err() {
        drop(destination);
        let _ = fs::remove_file(dst);
    } else {
        File::open(dst.parent().unwrap_or(Path::new(".")))?.sync_all()?;
    }
    result
}

fn ensure_free_space(file: &File, required: u64) -> Result<()> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stats = unsafe { stats.assume_init() };
    let available = (stats.f_bavail as u128) * (stats.f_frsize as u128);
    if available < required as u128 {
        return Err(Error::Invalid(format!(
            "insufficient free space: need {required} bytes, have {available} bytes"
        )));
    }
    Ok(())
}

pub fn lock(directory: &Path) -> Result<File> {
    private_dir(directory)?;
    let path = directory.join(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
        return Err(Error::Invalid(format!(
            "{} is not a private lock file",
            path.display()
        )));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Record {
        value: String,
    }

    #[test]
    fn private_directory_is_created_and_unsafe_directories_are_rejected() {
        let root = tempdir().unwrap();
        let private = root.path().join("private");
        private_dir(&private).unwrap();
        let metadata = fs::metadata(&private).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o700);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });

        let unsafe_dir = root.path().join("unsafe");
        fs::create_dir(&unsafe_dir).unwrap();
        fs::set_permissions(&unsafe_dir, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(private_dir(&unsafe_dir).is_err());
        let link = root.path().join("link");
        symlink(&private, &link).unwrap();
        assert!(private_dir(&link).is_err());
    }

    #[test]
    fn atomic_json_replaces_with_private_expected_content() {
        let root = tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(&path, b"old").unwrap();
        atomic_json(
            &path,
            &Record {
                value: "new".into(),
            },
        )
        .unwrap();
        assert_eq!(
            read_json::<Record>(&path).unwrap(),
            Record {
                value: "new".into()
            }
        );
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn copy_disk_makes_independent_asymmetric_copies() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::write(&source, b"left\0\0\0right").unwrap();
        copy_disk(&source, &first, Instant::now() + Duration::from_secs(2)).unwrap();
        fs::write(&source, b"changed").unwrap();
        copy_disk(&source, &second, Instant::now() + Duration::from_secs(2)).unwrap();
        assert_eq!(fs::read(first).unwrap(), b"left\0\0\0right");
        assert_eq!(fs::read(second).unwrap(), b"changed");
    }

    #[test]
    fn copy_rejects_existing_symlinks_and_nonregular_sources() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        fs::write(&source, b"data").unwrap();
        fs::write(&destination, b"existing").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        assert!(copy_disk(&source, &destination, deadline).is_err());
        fs::remove_file(&destination).unwrap();
        symlink(&source, &destination).unwrap();
        assert!(copy_disk(&source, &destination, deadline).is_err());
        fs::remove_file(&destination).unwrap();
        assert!(copy_disk(root.path(), &destination, deadline).is_err());
        let source_link = root.path().join("source-link");
        symlink(&source, &source_link).unwrap();
        assert!(copy_disk(&source_link, &destination, deadline).is_err());
    }

    #[test]
    fn expired_copy_removes_output() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        fs::write(&source, vec![1; 64 * 1024]).unwrap();
        assert!(copy_disk(&source, &destination, Instant::now()).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn lock_is_exclusive_for_guard_lifetime() {
        let root = tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        private_dir(root.path()).unwrap();
        let first = lock(root.path()).unwrap();
        assert!(lock(root.path()).is_err());
        drop(first);
        lock(root.path()).unwrap();
    }

    #[test]
    fn validates_runtime_ids() {
        assert!(valid_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!valid_id("0123456789abcdef"));
        assert!(!valid_id("g123456789abcdef0123456789abcdef"));
    }
}
