//! Immutable snapshot artifacts. The private manifest authenticates the digest;
//! fs-verity enforces it on reads. Neither timestamps nor file modes are a seal.
use crate::{Error, Result, image};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seal {
    // Bind the fast-path metadata to the existing full-file hash. Both were
    // measured from the same immutable descriptor before manifest publication.
    pub sha256: String,
    pub digest: String,
}

// Linux uapi/linux/fsverity.h ABI on the runtime's supported x86_64 host.
const FS_IOC_ENABLE_VERITY: libc::c_ulong = 0x4080_6685;
const FS_IOC_MEASURE_VERITY: libc::c_ulong = 0xc004_6686;

#[repr(C)]
#[derive(Default)]
struct EnableArg {
    version: u32,
    hash_algorithm: u32,
    block_size: u32,
    salt_size: u32,
    salt_ptr: u64,
    sig_size: u32,
    reserved1: u32,
    sig_ptr: u64,
    reserved2: [u64; 11],
}

#[repr(C)]
struct Digest {
    algorithm: u16,
    size: u16,
    bytes: [u8; 32],
}

fn open(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Invalid(
            "verity artifact must be a regular file".into(),
        ));
    }
    Ok(file)
}

fn unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOTTY | libc::EOPNOTSUPP))
}

fn measure(file: &File) -> Result<String> {
    let mut digest = Digest {
        algorithm: 0,
        size: 32,
        bytes: [0; 32],
    };
    // SAFETY: Digest contains the UAPI header followed by the advertised 32-byte
    // output buffer, and remains valid for the synchronous ioctl.
    if unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_MEASURE_VERITY, &mut digest) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if digest.algorithm != 1 || digest.size != 32 {
        return Err(Error::Invalid("unsupported fs-verity digest".into()));
    }
    Ok(digest.bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Called only for new snapshot artifacts, before publishing their manifest.
/// Unsupported filesystems retain full SHA-256 verification on every launch.
pub fn seal(path: &Path) -> Result<(String, Option<Seal>)> {
    let mut file = open(path)?;
    let args = EnableArg {
        version: 1,
        hash_algorithm: 1,
        block_size: 4096,
        ..Default::default()
    };
    // SAFETY: EnableArg is the 128-byte UAPI structure, with reserved fields zero.
    // The fd is read-only; the kernel excludes writers during/after sealing.
    let enabled = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_ENABLE_VERITY, &args) } == 0;
    let digest = if enabled {
        Some(measure(&file)?)
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EEXIST) {
            Some(measure(&file)?)
        } else if unsupported(&error) {
            None
        } else {
            return Err(error.into());
        }
    };
    // Hash AFTER enabling, through the same descriptor. A writer must never
    // change the file between recording its content hash and its verity digest.
    let sha256 = image::sha256_reader(&mut file)?;
    file.sync_all()?;
    let seal = digest.map(|digest| Seal {
        sha256: sha256.clone(),
        digest,
    });
    Ok((sha256, seal))
}

pub fn verify(path: &Path, expected_sha256: &str, seal: Option<&Seal>) -> Result<()> {
    if let Some(seal) = seal {
        verify_file(&open(path)?, expected_sha256, seal)?;
    } else if image::sha256(path)? != expected_sha256 {
        return Err(Error::Invalid("artifact checksum mismatch".into()));
    }
    Ok(())
}

pub(super) fn verify_file(file: &File, expected_sha256: &str, seal: &Seal) -> Result<()> {
    if seal.sha256 != expected_sha256 {
        return Err(Error::Invalid("seal SHA-256 mismatch".into()));
    }
    // Missing verity, a different digest, or any I/O error is fatal. Do not
    // downgrade to a checksum scan, even if the replacement has equal bytes.
    if measure(file)? != seal.digest {
        return Err(Error::Invalid("fs-verity digest mismatch".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    // Independently known SHA-256 of ASCII "abc".
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn legacy_verification_rejects_equal_length_corruption() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        verify(&path, ABC, None).unwrap();
        fs::write(&path, b"abd").unwrap();
        assert!(verify(&path, ABC, None).is_err());
    }

    #[test]
    fn claimed_seal_never_falls_back_to_a_matching_content_hash() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        let seal = Seal {
            sha256: ABC.into(),
            digest: "00".repeat(32),
        };
        assert!(verify(&path, ABC, Some(&seal)).is_err());
    }

    #[test]
    fn seal_must_agree_with_manifest_content_hash() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        let seal = Seal {
            sha256: "11".repeat(32),
            digest: "22".repeat(32),
        };
        let error = verify(&path, ABC, Some(&seal)).unwrap_err().to_string();
        assert!(error.contains("seal SHA-256 mismatch"), "{error}");
    }

    #[test]
    fn only_unsupported_capabilities_allow_unsealed_publication() {
        for errno in [libc::ENOTTY, libc::EOPNOTSUPP] {
            assert!(unsupported(&std::io::Error::from_raw_os_error(errno)));
        }
        for errno in [
            libc::EPERM,
            libc::EACCES,
            libc::EINVAL,
            libc::EIO,
            libc::ENOSPC,
            libc::ETXTBSY,
        ] {
            assert!(!unsupported(&std::io::Error::from_raw_os_error(errno)));
        }
    }

    #[test]
    fn capture_hashes_the_artifact_even_when_sealing_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        let (hash, seal) = seal(&path).unwrap();
        assert_eq!(hash, ABC);
        verify(&path, &hash, seal.as_ref()).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(super::seal(&link).is_err());
        assert!(super::seal(root.path()).is_err());
    }

    #[test]
    #[ignore = "requires BOXD_TEST_VERITY_DIR on an fs-verity-enabled filesystem"]
    fn real_seal_enforces_immutability_and_rejects_replacements() {
        let parent = std::env::var("BOXD_TEST_VERITY_DIR").expect("set BOXD_TEST_VERITY_DIR");
        let root = tempfile::tempdir_in(parent).unwrap();
        let path = root.path().join("artifact");
        fs::write(&path, b"abc").unwrap();
        let (hash, seal) = seal(&path).unwrap();
        let seal = seal.expect("fs-verity must be enabled; fallback is not a pass");
        assert_eq!(hash, ABC);
        // Independently computed from the documented 256-byte descriptor and
        // SHA-256 of "abc" padded to one 4096-byte block (no salt).
        assert_eq!(
            seal.digest,
            "700b6bd8510f0b4f9bac8b9cf0459151a1c4a99f467892bb4bd289a67df8e19c"
        );
        verify(&path, ABC, Some(&seal)).unwrap();
        assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
        let copy = root.path().join("writable-disk");
        crate::storage::copy_disk(&path, &copy, Instant::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"abc");
        fs::write(&copy, b"xyz").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"abc").unwrap();
        assert!(
            verify(&path, ABC, Some(&seal)).is_err(),
            "unsealed replacement must fail even with identical bytes"
        );
        fs::write(&path, b"abd").unwrap();
        super::seal(&path)
            .unwrap()
            .1
            .expect("replacement must also be sealed");
        assert!(
            verify(&path, ABC, Some(&seal)).is_err(),
            "a different valid seal must not be accepted"
        );
    }
}
