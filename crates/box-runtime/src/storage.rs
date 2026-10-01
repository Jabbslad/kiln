use crate::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub mod overlay;
pub mod verity;

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
    copy_file(src, dst, deadline, None)
}

/// Materialize a snapshot disk after manifest verification. Sealed sources must
/// be read through their authenticated inode, including every logical zero.
pub fn copy_snapshot_disk(
    src: &Path,
    dst: &Path,
    expected: &str,
    seal: Option<&verity::Seal>,
    deadline: Instant,
) -> Result<CopyMethod> {
    copy_file(src, dst, deadline, seal.map(|seal| (expected, seal)))
}

fn copy_file(
    src: &Path,
    dst: &Path,
    deadline: Instant,
    sealed: Option<(&str, &verity::Seal)>,
) -> Result<CopyMethod> {
    if Instant::now() >= deadline {
        return Err(Error::Invalid("disk copy deadline expired".into()));
    }
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(src)?;
    let source_metadata = source.metadata()?;
    if !source_metadata.file_type().is_file() || source_metadata.file_type().is_block_device() {
        return Err(Error::Invalid(format!(
            "{} is not a regular file",
            src.display()
        )));
    }
    if let Some((expected, seal)) = sealed {
        verity::verify_file(&source, expected, seal)?;
    }
    let mut destination = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dst)?;

    let result = (|| -> Result<CopyMethod> {
        // A writable reflink does not inherit fs-verity. Only the read path of
        // the sealed source authenticates its blocks against the trusted digest.
        if sealed.is_none() {
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
        }

        ensure_free_space(&destination, source_metadata.len())?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        let mut offset = 0;
        let length = source_metadata.len();
        let mut seek_holes = sealed.is_none();
        while offset < length {
            if Instant::now() >= deadline {
                return Err(Error::Invalid("disk copy deadline expired".into()));
            }
            let mut end = length;
            if seek_holes {
                // Linux permits conservative hole reporting (the whole file
                // may be data). Unsupported filesystems retain zero scanning.
                let data =
                    unsafe { libc::lseek(source.as_raw_fd(), offset as i64, libc::SEEK_DATA) };
                if data < 0 {
                    let error = std::io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::ENXIO) => break, // trailing hole, including an all-hole file
                        Some(libc::EINVAL | libc::EOPNOTSUPP) => seek_holes = false,
                        _ => return Err(error.into()),
                    }
                } else {
                    let hole = unsafe { libc::lseek(source.as_raw_fd(), data, libc::SEEK_HOLE) };
                    if hole < 0 {
                        let error = std::io::Error::last_os_error();
                        match error.raw_os_error() {
                            Some(libc::EINVAL | libc::EOPNOTSUPP) => seek_holes = false,
                            _ => return Err(error.into()),
                        }
                    } else {
                        offset = data as u64;
                        end = (hole as u64).min(length);
                    }
                }
            }
            source.seek(SeekFrom::Start(offset))?;
            destination.seek(SeekFrom::Start(offset))?;
            while offset < end {
                if Instant::now() >= deadline {
                    return Err(Error::Invalid("disk copy deadline expired".into()));
                }
                let count = (end - offset).min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..count])?;
                if is_zero(&buffer[..count]) {
                    destination.seek(SeekFrom::Current(count as i64))?;
                } else {
                    destination.write_all(&buffer[..count])?;
                }
                offset += count as u64;
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

fn is_zero(bytes: &[u8]) -> bool {
    // Fixed-size comparisons let LLVM use wide loads rather than branching on
    // each byte. Check the short tail too; these are already-read bytes only.
    let mut chunks = bytes.chunks_exact(32);
    chunks.by_ref().all(|chunk| chunk == [0; 32])
        && chunks.remainder().iter().all(|byte| *byte == 0)
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

    #[test]
    fn zero_detection_checks_chunk_boundaries_and_partial_tails() {
        for length in [0, 1, 31, 32, 33, 63, 64, 65, 97] {
            // Misalign the input and place the only nonzero byte at every
            // possible position, including the portion after the last chunk.
            let mut bytes = vec![0; length + 1];
            assert!(is_zero(&bytes[1..]));
            for offset in 1..=length {
                bytes[offset] = 0x80;
                assert!(!is_zero(&bytes[1..]), "length {length}, offset {offset}");
                bytes[offset] = 0;
            }
        }
    }

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
    fn sealed_disk_copy_rejects_a_false_seal_before_creating_output() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("disk");
        fs::write(&source, b"abc").unwrap();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let seal = verity::Seal {
            sha256: hash.into(),
            digest: "00".repeat(32),
        };
        assert!(
            copy_snapshot_disk(
                &source,
                &target,
                hash,
                Some(&seal),
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
        assert!(!target.exists());
    }

    #[test]
    #[ignore = "requires BOXD_TEST_VERITY_DIR on Btrfs with fs-verity and reflinks"]
    fn sealed_disk_copy_reads_data_instead_of_reflinking_away_verification() {
        let root = tempdir_in_verity();
        let source = root.path().join("source");
        let target = root.path().join("disk");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut expected = vec![0; 8 * 1024 * 1024 + 31];
        expected[8193..8197].copy_from_slice(b"left");
        expected[8 * 1024 * 1024 + 17..8 * 1024 * 1024 + 22].copy_from_slice(b"right");
        let mut file = File::create(&source).unwrap();
        file.seek(SeekFrom::Start(8193)).unwrap();
        file.write_all(b"left").unwrap();
        file.seek(SeekFrom::Start(8 * 1024 * 1024 + 17)).unwrap();
        file.write_all(b"right").unwrap();
        file.set_len(expected.len() as u64).unwrap();
        drop(file);
        let (hash, seal) = verity::seal(&source).unwrap();
        let seal = seal.expect("real sealing required");
        // Prove FICLONE is available for this sealed source. Merely testing on
        // ext4 would let an unsafe implementation pass via unsupported fallback.
        let raw = root.path().join("raw-reflink");
        assert_eq!(
            copy_disk(&source, &raw, deadline).unwrap(),
            CopyMethod::Reflink
        );
        let rchar = || -> u64 {
            fs::read_to_string("/proc/thread-self/io")
                .unwrap()
                .lines()
                .find_map(|line| line.strip_prefix("rchar: "))
                .unwrap()
                .parse()
                .unwrap()
        };
        let before = rchar();
        assert_eq!(
            copy_snapshot_disk(&source, &target, &hash, Some(&seal), deadline).unwrap(),
            CopyMethod::Copy
        );
        assert!(
            rchar() - before >= expected.len() as u64,
            "must read holes too, not synthesize unauthenticated zeroes"
        );
        assert_eq!(fs::read(&target).unwrap(), expected);
        fs::write(&target, b"changed").unwrap();
        assert_eq!(fs::read(&source).unwrap(), expected);
        fs::remove_file(&source).unwrap();
        fs::write(&source, b"replacement").unwrap();
        verity::seal(&source).unwrap().1.expect("seal replacement");
        let rejected = root.path().join("rejected");
        assert!(copy_snapshot_disk(&source, &rejected, &hash, Some(&seal), deadline).is_err());
        assert!(!rejected.exists());
    }

    fn tempdir_in_verity() -> tempfile::TempDir {
        tempfile::tempdir_in(
            std::env::var("BOXD_TEST_VERITY_DIR").expect("set BOXD_TEST_VERITY_DIR"),
        )
        .unwrap()
    }

    #[test]
    fn sparse_copy_preserves_extents_holes_and_partial_tail() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        let mut file = File::create(&source).unwrap();
        let mut expected = vec![0; 5 * 1024 * 1024 + 31];
        for (offset, bytes) in [
            (8193, b"left".as_slice()),
            (3 * 1024 * 1024 + 17, b"right-tail"),
        ] {
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            file.write_all(bytes).unwrap();
            expected[offset..offset + bytes.len()].copy_from_slice(bytes);
        }
        file.set_len(expected.len() as u64).unwrap();
        file.sync_all().unwrap();
        copy_disk(
            &source,
            &destination,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), expected);
        // The old 1 MiB zero scanning copies an entire chunk for each tiny
        // extent. SEEK_DATA/HOLE should preserve the large surrounding holes.
        if unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_DATA) } > 0 {
            assert!(fs::metadata(&destination).unwrap().blocks() * 512 < 128 * 1024);
        }
        fs::write(&source, b"changed").unwrap();
        assert_eq!(fs::read(destination).unwrap(), expected);
    }

    #[test]
    fn copy_handles_empty_all_hole_and_dense_files_with_short_final_block() {
        let root = tempdir().unwrap();
        let mut dense = vec![0; 2 * 1024 * 1024 + 71];
        for (index, byte) in dense.iter_mut().enumerate().skip(1024 * 1024) {
            *byte = (index % 251) as u8;
        }
        for (index, expected) in [vec![], vec![0; 65539], dense].into_iter().enumerate() {
            let source = root.path().join(format!("source-{index}"));
            let destination = root.path().join(format!("destination-{index}"));
            let mut file = File::create(&source).unwrap();
            if index == 1 {
                file.set_len(expected.len() as u64).unwrap();
            } else {
                file.write_all(&expected).unwrap();
            }
            file.sync_all().unwrap();
            copy_disk(
                &source,
                &destination,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(fs::read(destination).unwrap(), expected);
        }
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
    fn fifo_source_is_rejected_without_waiting_for_a_writer() {
        let root = tempdir().unwrap();
        let source = root.path().join("fifo");
        let destination = root.path().join("copy");
        let name = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        let input = source.clone();
        let worker = std::thread::spawn(move || {
            send.send(
                copy_disk(
                    &input,
                    &destination,
                    Instant::now() + Duration::from_secs(3),
                )
                .is_err(),
            )
            .unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(1));
        if result.is_err() {
            // Release an incorrectly blocking open before failing the assertion.
            let _writer = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&source)
                .unwrap();
        }
        worker.join().unwrap();
        assert!(result.unwrap());
    }

    #[test]
    fn lock_is_exclusive_for_guard_lifetime() {
        let root = tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        private_dir(root.path()).unwrap();
        let first = lock(root.path()).unwrap();
        assert!(lock(root.path()).is_err());
        drop(first);
        // Parallel tests spawn helpers. Between fork and exec, a child briefly
        // inherits even CLOEXEC lock descriptors; release follows its exec.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match lock(root.path()) {
                Ok(_) => break,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "lock was not released");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("unexpected lock error: {error}"),
            }
        }
    }

    #[test]
    fn validates_runtime_ids() {
        assert!(valid_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!valid_id("0123456789abcdef"));
        assert!(!valid_id("g123456789abcdef0123456789abcdef"));
    }
}
