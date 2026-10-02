use crate::{Error, Result, process::FileIdentity};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};

// Linux uapi/linux/loop.h, x86_64. Atomic LOOP_CONFIGURE avoids a window
// where a manager crash leaves an attached loop without AUTOCLEAR.
#[repr(C)]
struct Info {
    device: u64,
    inode: u64,
    rdevice: u64,
    offset: u64,
    size_limit: u64,
    number: u32,
    encrypt_type: u32,
    key_size: u32,
    flags: u32,
    name: [u8; 64],
    crypt_name: [u8; 64],
    key: [u8; 32],
    init: [u64; 2],
}

#[repr(C)]
struct Config {
    fd: u32,
    block_size: u32,
    info: Info,
    reserved: [u64; 8],
}

fn info() -> Info {
    // All fields are integers/byte arrays; the UAPI requires unused fields zero.
    unsafe { std::mem::zeroed() }
}

fn label(name: &str) -> Result<[u8; 64]> {
    if name.len() >= 64 || name.as_bytes().contains(&0) {
        return Err(Error::Invalid("invalid loop reference".into()));
    }
    let mut bytes = [0; 64];
    bytes[..name.len()].copy_from_slice(name.as_bytes());
    Ok(bytes)
}

pub(super) fn node(path: &Path, device: u64, uid: u32, gid: u32) -> Result<()> {
    if path.try_exists()? {
        let m = fs::symlink_metadata(path)?;
        if !m.file_type().is_block_device()
            || m.rdev() != device
            || m.uid() != uid
            || m.gid() != gid
            || m.mode() & 0o7777 != 0o600
        {
            return Err(Error::Invalid(
                "block node identity or ownership mismatch".into(),
            ));
        }
        return Ok(());
    }
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Invalid("invalid node path".into()))?;
    if unsafe { libc::mknod(name.as_ptr(), libc::S_IFBLK | 0o600, device) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe { libc::chown(name.as_ptr(), uid, gid) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

// These private root-owned helper nodes are not exposed to a jail. Replacing
// a stale node changes only the directory entry, never the referenced device.
pub(super) fn helper_node(path: &Path, device: u64) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_block_device() && m.uid() == 0 && m.mode() & 0o7777 == 0o600 => {
            if m.rdev() == device && m.gid() == 0 {
                return Ok(());
            }
            fs::remove_file(path)?
        }
        Ok(_) => return Err(Error::Invalid("invalid loop helper node".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    node(path, device, 0, 0)
}

pub(super) fn attach(
    backing: &File,
    node_path: &Path,
    name: &str,
    readonly: bool,
) -> Result<(File, u32)> {
    let control = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/loop-control")?;
    for _ in 0..16 {
        let minor = unsafe { libc::ioctl(control.as_raw_fd(), 0x4c82) };
        if minor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        helper_node(node_path, libc::makedev(7, minor as u32))?;
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(node_path)?;
        let mut config = Config {
            fd: backing.as_raw_fd() as u32,
            block_size: 512,
            info: info(),
            reserved: [0; 8],
        };
        config.info.flags = 4 | u32::from(readonly); // AUTOCLEAR; no direct I/O or partition scan
        config.info.name = label(name)?;
        if unsafe { libc::ioctl(device.as_raw_fd(), 0x4c0a, &config) } == 0 {
            return Ok((device, minor as u32));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EBUSY) {
            return Err(error.into());
        }
    }
    Err(Error::Invalid("loop allocation remained busy".into()))
}

pub(super) fn verify(
    node_path: &Path,
    minor: u32,
    name: &str,
    expected: &FileIdentity,
    readonly: bool,
) -> Result<()> {
    helper_node(node_path, libc::makedev(7, minor))?;
    let device = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(node_path)?;
    let mut actual = info();
    if unsafe { libc::ioctl(device.as_raw_fd(), 0x4c05, &mut actual) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if actual.device != expected.device
        || actual.inode != expected.inode
        || actual.offset != 0
        || actual.size_limit != 0
        || actual.number != minor
        || actual.flags != 4 | u32::from(readonly)
        || actual.name != label(name)?
        || actual.encrypt_type != 0
    {
        return Err(Error::Invalid(
            "loop backing identity or flags mismatch".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loop_uapi_layout_and_labels() {
        assert_eq!(std::mem::size_of::<Info>(), 232);
        assert_eq!(std::mem::size_of::<Config>(), 304);
        assert_eq!(&label("kiln-test").unwrap()[..10], b"kiln-test\0");
        assert!(label(&"x".repeat(64)).is_err());
        assert!(label("a\0b").is_err());
    }
}
