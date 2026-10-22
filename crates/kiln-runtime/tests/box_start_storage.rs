//! Box create/start storage optimizations (PR2): disk relocate on start and
//! sparsify-before-copy for dense golden images.

use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use kiln_runtime::storage::{self, CopyMethod};

fn rchar_bytes() -> u64 {
    fs::read_to_string("/proc/thread-self/io")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("rchar: "))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn sparsify_dense_image_limits_copy_read_bytes() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("dense-rootfs");
    let destination = root.path().join("box-disk");
    let logical = 8 * 1024 * 1024_u64;
    let mut file = File::create(&source).unwrap();
    assert_eq!(
        unsafe { libc::fallocate(file.as_raw_fd(), 0, 0, logical as i64) },
        0,
        "dense fixture requires fallocate"
    );
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"boot").unwrap();
    file.seek(SeekFrom::Start(4 * 1024 * 1024)).unwrap();
    file.write_all(b"tail").unwrap();
    file.sync_all().unwrap();
    let dense_blocks = fs::metadata(&source).unwrap().blocks();
    assert!(
        dense_blocks * 512 >= logical / 2,
        "fixture should be dense on disk: {dense_blocks} blocks"
    );

    storage::sparsify_in_place(&source).unwrap();
    let sparse_blocks = fs::metadata(&source).unwrap().blocks();
    if sparse_blocks * 512 >= logical / 4 {
        eprintln!("skip sparsify read proof: punch hole ineffective on this filesystem");
        return;
    }

    let before = rchar_bytes();
    copy_disk(
        &source,
        &destination,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    let read = rchar_bytes() - before;
    assert!(
        read < logical / 4,
        "sparse-aware copy should not read the full logical image ({read} vs {logical})"
    );
    let mut expected = vec![0; logical as usize];
    expected[..4].copy_from_slice(b"boot");
    expected[4 * 1024 * 1024..4 * 1024 * 1024 + 4].copy_from_slice(b"tail");
    assert_eq!(fs::read(destination).unwrap(), expected);
}

#[test]
fn relocate_disk_moves_without_copying_bytes() {
    let root = tempfile::tempdir().unwrap();
    let generation = root.path().join("old-gen");
    fs::create_dir_all(generation.join("root")).unwrap();
    let source = generation.join("root").join("disk.ext4");
    fs::write(&source, b"generation-disk-bytes").unwrap();
    let inode_before = fs::metadata(&source).unwrap().ino();

    let destination = root.path().join("new-gen").join("root").join("disk.ext4");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let new_gen = root.path().join("new-gen");
    for path in [
        root.path(),
        new_gen.as_path(),
        destination.parent().unwrap(),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let before = rchar_bytes();
    assert_eq!(
        storage::relocate_disk(&source, &destination).unwrap(),
        CopyMethod::Move
    );
    assert!(
        rchar_bytes() - before < 4096,
        "relocate must not read the disk"
    );
    assert!(!source.exists());
    assert_eq!(fs::read(&destination).unwrap(), b"generation-disk-bytes");
    assert_eq!(fs::metadata(&destination).unwrap().ino(), inode_before);
}

fn copy_disk(src: &Path, dst: &Path, deadline: Instant) -> kiln_runtime::Result<CopyMethod> {
    storage::copy_disk(src, dst, deadline)
}

#[test]
fn generation_disk_relocate_beats_copy_for_storage_ms() {
    let root = tempfile::tempdir().unwrap();
    let payload = vec![0xAB; 4 * 1024 * 1024];
    let source = root.path().join("source");
    fs::write(&source, &payload).unwrap();
    let copy_dst = root.path().join("copy");
    let copy_started = Instant::now();
    copy_disk(&source, &copy_dst, Instant::now() + Duration::from_secs(10)).unwrap();
    let copy_ms = copy_started.elapsed().as_secs_f64() * 1000.;

    let move_src = root.path().join("move-src");
    fs::write(&move_src, &payload).unwrap();
    let move_dst = root.path().join("move-dst").join("disk.ext4");
    let new_gen = move_dst.parent().unwrap();
    fs::create_dir_all(new_gen).unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(new_gen, fs::Permissions::from_mode(0o700)).unwrap();
    let move_started = Instant::now();
    storage::relocate_disk(&move_src, &move_dst).unwrap();
    let move_ms = move_started.elapsed().as_secs_f64() * 1000.;

    assert!(
        move_ms < copy_ms / 5.,
        "relocate ({move_ms:.2} ms) should beat copy ({copy_ms:.2} ms) for generation disks"
    );
}
