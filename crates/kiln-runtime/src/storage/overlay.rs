//! Persistent device-mapper snapshots over authenticated, immutable base files.

use super::{atomic_json, private_dir, read_json, valid_id, verity};
use crate::{Error, Result, isolation, process::FileIdentity};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::Path,
    process::Command,
    time::Instant,
};

mod loop_device;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub source: String,
    size: u64,
    base: FileIdentity,
    cow: FileIdentity,
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

fn identity(file: &File) -> Result<FileIdentity> {
    let metadata = file.metadata()?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn cow_size(size: u64) -> Result<u64> {
    if size == 0 || !size.is_multiple_of(512) || size > 2 * 1024 * 1024 * 1024 {
        return Err(invalid(
            "overlay bases must be sector-aligned and at most 2 GiB",
        ));
    }
    // 4 KiB chunks, 16-byte persistent exception records, plus spare metadata.
    Ok((size + size / 128 + 1024 * 1024).next_multiple_of(4096))
}

fn name(directory: &Path) -> Result<String> {
    let id = directory
        .file_name()
        .and_then(|v| v.to_str())
        .filter(|id| valid_id(id))
        .ok_or_else(|| invalid("invalid layer ID"))?;
    Ok(format!("kiln-{id}"))
}

fn uuid(directory: &Path) -> Result<String> {
    Ok(format!("KILN-SNAPSHOT-{}", name(directory)?))
}

pub fn preflight() -> Result<()> {
    isolation::require_root()?;
    isolation::trusted_path(Path::new("/usr/sbin/dmsetup"))?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/loop-control")?;
    Ok(())
}

fn dm(args: &[&str]) -> Result<String> {
    dm_with_devices(args, &[])
}

fn inherit_devices(command: &mut Command, devices: &[&File]) {
    let descriptors: Vec<_> = devices.iter().map(|file| file.as_raw_fd()).collect();
    // Only async-signal-safe fcntl calls occur between fork and exec. The child
    // pins loops even if its manager dies before the kernel loads the dm table.
    unsafe {
        command.pre_exec(move || {
            for fd in &descriptors {
                if libc::fcntl(*fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

fn dm_with_devices(args: &[&str], devices: &[&File]) -> Result<String> {
    isolation::trusted_path(Path::new("/usr/sbin/dmsetup"))?;
    let mut command = Command::new("/usr/sbin/dmsetup");
    command
        .arg("--noudevsync")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    inherit_devices(&mut command, devices);
    let output = command.output()?;
    if !output.status.success() {
        return Err(Error::Invalid(format!(
            "dmsetup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| invalid("invalid mapper output"))
}

fn regular(path: &Path, writable: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid(
            "overlay backing must be a private root-owned single-link file",
        ));
    }
    Ok(file)
}

pub fn create(
    directory: &Path,
    source: &str,
    base: &Path,
    hash: &str,
    seal: &verity::Seal,
) -> Result<Layer> {
    preflight()?;
    name(directory)?;
    if !valid_id(source) {
        return Err(invalid("invalid layer base reference"));
    }
    private_dir(directory)?;
    let base = regular(base, false)?;
    verity::verify_file(&base, hash, seal)?;
    let size = base.metadata()?.len();
    let length = cow_size(size)?;
    let cow = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("cow"))?;
    super::ensure_free_space(&cow, length)?;
    cow.set_len(length)?;
    cow.sync_all()?;
    let layer = Layer {
        source: source.into(),
        size,
        base: identity(&base)?,
        cow: identity(&cow)?,
    };
    atomic_json(&directory.join("layer.json"), &layer)?;
    Ok(layer)
}

pub fn read(directory: &Path) -> Result<Layer> {
    name(directory)?;
    private_dir(directory)?;
    let layer: Layer = read_json(&directory.join("layer.json"))?;
    if !valid_id(&layer.source) {
        return Err(invalid("invalid layer base reference"));
    }
    cow_size(layer.size)?;
    Ok(layer)
}

fn mapper(directory: &Path) -> Result<Option<u64>> {
    let expected = name(directory)?;
    for entry in fs::read_dir("/sys/block")? {
        let path = entry?.path();
        if !path.join("dm").is_dir() {
            continue;
        }
        let actual_name = match fs::read_to_string(path.join("dm/name")) {
            Ok(name) => name,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if actual_name.trim() != expected {
            continue;
        }
        if fs::read_to_string(path.join("dm/uuid"))?.trim() != uuid(directory)? {
            return Err(invalid("mapper name is owned by a different UUID"));
        }
        return Ok(Some(parse_device(
            fs::read_to_string(path.join("dev"))?.trim(),
        )?));
    }
    Ok(None)
}

fn parse_device(text: &str) -> Result<u64> {
    let (major, minor) = text
        .split_once(':')
        .ok_or_else(|| invalid("invalid device number"))?;
    Ok(libc::makedev(
        major.parse().map_err(|_| invalid("invalid device major"))?,
        minor.parse().map_err(|_| invalid("invalid device minor"))?,
    ))
}

fn validate_table(table: &str, size: u64, base: u32, cow: u32) -> Result<()> {
    let expected = format!("0 {} snapshot 7:{base} 7:{cow} P 8", size / 512);
    if table.split_whitespace().collect::<Vec<_>>()
        != expected.split_whitespace().collect::<Vec<_>>()
    {
        return Err(invalid(
            "mapper table differs from owned persistent snapshot",
        ));
    }
    Ok(())
}

fn validate_status(status: &str, size: u64) -> Result<()> {
    let parts: Vec<_> = status.split_whitespace().collect();
    if parts.len() != 5
        || parts[0] != "0"
        || parts[1] != (size / 512).to_string()
        || parts[2] != "snapshot"
    {
        return Err(invalid(
            "snapshot is invalid, overflowed, or has unexpected status",
        ));
    }
    let (used, capacity) = parts[3]
        .split_once('/')
        .ok_or_else(|| invalid("invalid snapshot usage"))?;
    let used: u64 = used
        .parse()
        .map_err(|_| invalid("invalid snapshot usage"))?;
    let capacity: u64 = capacity
        .parse()
        .map_err(|_| invalid("invalid snapshot capacity"))?;
    if used >= capacity {
        return Err(invalid("snapshot COW capacity exhausted"));
    }
    Ok(())
}

fn verify_mapping(directory: &Path, layer: &Layer) -> Result<u64> {
    let device = mapper(directory)?.ok_or_else(|| invalid("owned mapper missing"))?;
    let name = name(directory)?;
    let table = dm(&["table", &name])?;
    let fields: Vec<_> = table.split_whitespace().collect();
    if fields.len() != 7 {
        return Err(invalid("unexpected snapshot table"));
    }
    let base = parse_device(fields[3])?;
    let cow = parse_device(fields[4])?;
    if libc::major(base) != 7 || libc::major(cow) != 7 || base == cow {
        return Err(invalid("snapshot does not use distinct owned loops"));
    }
    validate_table(&table, layer.size, libc::minor(base), libc::minor(cow))?;
    loop_device::verify(
        &directory.join("base.loop"),
        libc::minor(base),
        &format!("{name}-base"),
        &layer.base,
        true,
    )?;
    loop_device::verify(
        &directory.join("cow.loop"),
        libc::minor(cow),
        &format!("{name}-cow"),
        &layer.cow,
        false,
    )?;
    Ok(device)
}

pub fn ensure(directory: &Path, base: &Path, hash: &str, seal: &verity::Seal) -> Result<u64> {
    preflight()?;
    let lock = super::lock(directory)?;
    let layer = read(directory)?;
    let base = regular(base, false)?;
    verity::verify_file(&base, hash, seal)?;
    let cow = regular(&directory.join("cow"), true)?;
    if identity(&base)? != layer.base
        || identity(&cow)? != layer.cow
        || base.metadata()?.len() != layer.size
        || cow.metadata()?.len() != cow_size(layer.size)?
    {
        return Err(invalid("overlay backing identity or size changed"));
    }
    let name = name(directory)?;
    if mapper(directory)?.is_none() {
        let (base_fd, _) = loop_device::attach(
            &base,
            &directory.join("base.loop"),
            &format!("{name}-base"),
            true,
        )?;
        let (cow_fd, _) = loop_device::attach(
            &cow,
            &directory.join("cow.loop"),
            &format!("{name}-cow"),
            false,
        )?;
        let table = format!(
            "0 {} snapshot /proc/self/fd/{} /proc/self/fd/{} P 8",
            layer.size / 512,
            base_fd.as_raw_fd(),
            cow_fd.as_raw_fd()
        );
        // Hold both AUTOCLEAR loops until dm owns references. If allocation or
        // the manager fails first, kernel fd teardown detaches them automatically.
        dm_with_devices(
            &[
                "create",
                &name,
                "--uuid",
                &uuid(directory)?,
                "--mode",
                "0600",
                "--table",
                &table,
            ],
            &[&base_fd, &cow_fd, &lock],
        )?;
        drop((base_fd, cow_fd));
    }
    let device = verify_mapping(directory, &layer)?;
    validate_status(&dm(&["status", &name])?, layer.size)?;
    Ok(device)
}

pub fn check(directory: &Path) -> Result<u64> {
    let layer = read(directory)?;
    let device = verify_mapping(directory, &layer)?;
    validate_status(&dm(&["status", &name(directory)?])?, layer.size)?;
    Ok(device)
}

pub fn expose(directory: &Path, destination: &Path, uid: u32, gid: u32) -> Result<()> {
    loop_device::node(destination, check(directory)?, uid, gid)
}

pub fn detach(directory: &Path) -> Result<()> {
    // An orphaned creation/removal helper retains this lock until it exits.
    // A replacement manager must not delete backing files ahead of that helper.
    let lock = super::lock(directory)?;
    let layer = read(directory)?;
    if mapper(directory)?.is_some() {
        verify_mapping(directory, &layer)?;
        // Never force or defer removal. Busy storage leaves state for diagnosis.
        dm_with_devices(&["remove", &name(directory)?], &[&lock])?;
    }
    Ok(())
}

pub fn flatten(directory: &Path, destination: &Path, deadline: Instant) -> Result<()> {
    let layer = read(directory)?;
    let device = directory.join("flatten.device");
    loop_device::helper_node(&device, check(directory)?)?;
    let mut source = File::open(&device)?;
    fs::remove_file(&device)?;
    source.sync_all()?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(destination)?;
    let result = (|| -> Result<()> {
        super::ensure_free_space(&target, layer.size)?;
        let mut buffer = vec![0; 1024 * 1024];
        let mut remaining = layer.size;
        while remaining != 0 {
            if Instant::now() >= deadline {
                return Err(invalid("overlay flatten deadline expired"));
            }
            let length = remaining.min(buffer.len() as u64) as usize;
            source.read_exact(&mut buffer[..length])?;
            if super::is_zero(&buffer[..length]) {
                target.seek(SeekFrom::Current(length as i64))?;
            } else {
                target.write_all(&buffer[..length])?;
            }
            remaining -= length as u64;
        }
        target.set_len(layer.size)?;
        target.sync_all()?;
        File::open(destination.parent().unwrap())?.sync_all()?;
        validate_status(&dm(&["status", &name(directory)?])?, layer.size)
    })();
    if result.is_err() {
        fs::remove_file(destination)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_inherits_pinned_descriptors_until_it_exits() {
        use std::{os::fd::AsRawFd, process::Stdio};
        let file = tempfile::tempfile().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let lock_path = directory.path().join("private");
        let lock = super::super::lock(&lock_path).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!("read -r ready; test -f /proc/self/fd/{}", file.as_raw_fd()),
        ]);
        command.stdin(Stdio::piped());
        inherit_devices(&mut command, &[&file, &lock]);
        let mut child = command.spawn().unwrap();
        drop((file, lock));
        let locked = super::super::lock(&lock_path).is_err();
        child.stdin.take().unwrap().write_all(b"ready\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(
            locked,
            "helper must prevent deletion after its manager exits"
        );
        assert!(super::super::lock(&lock_path).is_ok());
    }

    #[test]
    #[ignore = "requires root, device mapper snapshot, and KILN_TEST_VERITY_DIR"]
    fn real_layers_are_private_persistent_and_cleanup_checks_ownership() {
        use std::{os::unix::fs::PermissionsExt, time::Duration};
        preflight().unwrap();
        let root = tempfile::tempdir_in(std::env::var("KILN_TEST_VERITY_DIR").unwrap())
            .unwrap()
            .keep();
        eprintln!(
            "overlay test state (retained on failure): {}",
            root.display()
        );
        let base = root.join("base");
        let mut expected = vec![0u8; 8 * 1024 * 1024];
        expected[8193..8197].copy_from_slice(b"left");
        expected[8 * 1024 * 1024 - 5..].copy_from_slice(b"right");
        fs::write(&base, &expected).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o600)).unwrap();
        let (hash, seal) = verity::seal(&base).unwrap();
        let seal = seal.expect("real sealing is required");
        let a = root.join(
            fs::read_to_string("/proc/sys/kernel/random/uuid")
                .unwrap()
                .trim()
                .replace('-', ""),
        );
        let b = root.join(
            fs::read_to_string("/proc/sys/kernel/random/uuid")
                .unwrap()
                .trim()
                .replace('-', ""),
        );
        let source = "123456789abcdef0123456789abcdef0";
        for path in [&a, &b] {
            create(path, source, &base, &hash, &seal).unwrap();
            ensure(path, &base, &hash, &seal).unwrap();
            expose(path, &path.join("guest"), 0, 0).unwrap();
        }
        for (path, bytes) in [(&a, b"alpha"), (&b, b"bravo")] {
            let mut disk = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path.join("guest"))
                .unwrap();
            disk.seek(SeekFrom::Start(8193)).unwrap();
            disk.write_all(bytes).unwrap();
            disk.sync_all().unwrap();
        }
        assert_eq!(fs::read(&base).unwrap(), expected);
        let original = read(&a).unwrap();
        let mut wrong = original.clone();
        wrong.cow.inode += 1;
        atomic_json(&a.join("layer.json"), &wrong).unwrap();
        assert!(detach(&a).is_err());
        assert!(mapper(&a).unwrap().is_some());
        atomic_json(&a.join("layer.json"), &original).unwrap();
        detach(&a).unwrap();
        assert!(mapper(&a).unwrap().is_none());
        ensure(&a, &base, &hash, &seal).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let flattened = root.join("flattened");
        flatten(&a, &flattened, deadline).unwrap();
        let mut a_expected = expected.clone();
        a_expected[8193..8198].copy_from_slice(b"alpha");
        assert_eq!(fs::read(&flattened).unwrap(), a_expected);
        detach(&a).unwrap();
        let other = root.join("other");
        flatten(&b, &other, deadline).unwrap();
        expected[8193..8198].copy_from_slice(b"bravo");
        assert_eq!(fs::read(&other).unwrap(), expected);
        let mut bad_seal = seal.clone();
        bad_seal.digest = "00".repeat(32);
        assert!(ensure(&b, &base, &hash, &bad_seal).is_err());
        detach(&b).unwrap();
        // A substituted mutable file must not be adopted even at the right size.
        fs::rename(b.join("cow"), b.join("original-cow")).unwrap();
        let substitute = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(b.join("cow"))
            .unwrap();
        substitute
            .set_len(cow_size(read(&b).unwrap().size).unwrap())
            .unwrap();
        assert!(ensure(&b, &base, &hash, &seal).is_err());
        assert!(mapper(&b).unwrap().is_none());
        drop(substitute);
        fs::remove_file(b.join("cow")).unwrap();
        fs::rename(b.join("original-cow"), b.join("cow")).unwrap();
        ensure(&b, &base, &hash, &seal).unwrap();
        detach(&b).unwrap();
        // Same name but different UUID is not ours, even if no VMM uses it.
        dm(&[
            "create",
            &name(&a).unwrap(),
            "--uuid",
            "foreign-test-device",
            "--table",
            "0 16384 error",
        ])
        .unwrap();
        assert!(detach(&a).is_err());
        assert!(
            dm(&["table", &name(&a).unwrap()])
                .unwrap()
                .contains("error")
        );
        // This synthetic foreign target emits uevents; a probe can briefly
        // hold it open. Bounded removal retry is only fixture teardown, not
        // permission for the runtime to force/defer removal of foreign devices.
        dm(&["remove", "--retry", &name(&a).unwrap()]).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mapper_table_and_capacity_are_exact_not_substring_matches() {
        let valid = "0 16384 snapshot 7:11 7:12 P 8";
        assert!(validate_table(valid, 8 * 1024 * 1024, 11, 12).is_ok());
        for table in [
            "1 16384 snapshot 7:11 7:12 P 8",
            "0 16383 snapshot 7:11 7:12 P 8",
            "0 16384 snapshot 7:12 7:11 P 8",
            "0 16384 snapshot 7:11 7:12 N 8",
            "0 16384 snapshot 7:11 7:12 P 16",
            "0 16384 snapshot 7:11 7:12 P 8\n0 1 error",
        ] {
            assert!(validate_table(table, 8 * 1024 * 1024, 11, 12).is_err());
        }
        assert!(cow_size(0).is_err());
        assert!(cow_size(513).is_err());
        assert!(cow_size(u64::MAX).is_err());
        assert!(cow_size(2 * 1024 * 1024 * 1024).unwrap() > 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn invalid_or_overflowed_snapshots_are_not_healthy() {
        assert!(validate_status("0 16384 snapshot 16/20000 8", 8 * 1024 * 1024).is_ok());
        for status in [
            "0 16384 snapshot Invalid",
            "0 16384 snapshot Overflow",
            "0 16384 snapshot 20000/20000 8",
            "0 16384 snapshot 20001/20000 8",
            "0 16384 linear 16/20000 8",
        ] {
            assert!(validate_status(status, 8 * 1024 * 1024).is_err());
        }
    }
}
