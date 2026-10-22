use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::{fs, time::SystemTime};

#[test]
fn list_leaves_box_json_bytes_and_mtime_unchanged() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let id = "a".repeat(32);
    let boxes_dir = root.path().join("boxes");
    fs::create_dir_all(&boxes_dir).unwrap();
    fs::set_permissions(boxes_dir.as_path(), fs::Permissions::from_mode(0o700)).unwrap();
    let box_dir = boxes_dir.join(&id);
    fs::create_dir_all(&box_dir).unwrap();
    fs::set_permissions(&box_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let box_json = box_dir.join("box.json");
    let payload = format!(
        r#"{{
  "schema_version": 1,
  "id": "{id}",
  "name": "health-check",
  "state": "stopped",
  "run": "{}",
  "image": {{
    "schema_version": 1,
    "architecture": "x86_64",
    "kernel_path": "kernel",
    "kernel_sha256": "{}",
    "rootfs_path": "rootfs",
    "rootfs_sha256": "{}",
    "agent_protocol_version": 1
  }},
  "host": "test-host",
  "memory_mib": 256,
  "vcpus": 1,
  "disk_copy": ""
}}"#,
        "c".repeat(32),
        "ab".repeat(32),
        "cd".repeat(32),
    );
    fs::write(&box_json, &payload).unwrap();
    let run_dir = box_dir.join("c".repeat(32));
    fs::create_dir_all(&run_dir).unwrap();
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let fake_bin = root.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let firecracker = fake_bin.join("firecracker");
    fs::write(&firecracker, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&firecracker, fs::Permissions::from_mode(0o755)).unwrap();
    let before_bytes = fs::read(&box_json).unwrap();
    let before_mtime = fs::metadata(&box_json).unwrap().modified().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .env("PATH", fake_bin)
        .arg("--state-dir")
        .arg(root.path())
        .arg("list")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_bytes = fs::read(&box_json).unwrap();
    let after_mtime = fs::metadata(&box_json).unwrap().modified().unwrap();
    assert_eq!(after_bytes, before_bytes, "list rewrote box.json contents");
    assert_eq!(
        after_mtime, before_mtime,
        "list touched box.json modification time"
    );
    // Give the filesystem a moment; a rewrite would usually bump mtime strictly forward.
    assert!(
        after_mtime <= SystemTime::now(),
        "unexpected box.json mtime after list"
    );
}

#[test]
fn inspect_on_stopped_box_leaves_box_json_unchanged() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let id = "b".repeat(32);
    let boxes_dir = root.path().join("boxes");
    fs::create_dir_all(&boxes_dir).unwrap();
    fs::set_permissions(boxes_dir.as_path(), fs::Permissions::from_mode(0o700)).unwrap();
    let box_dir = boxes_dir.join(&id);
    fs::create_dir_all(&box_dir).unwrap();
    fs::set_permissions(&box_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let box_json = box_dir.join("box.json");
    let payload = format!(
        r#"{{
  "schema_version": 1,
  "id": "{id}",
  "name": "inspect-readonly",
  "state": "stopped",
  "run": "{}",
  "image": {{
    "schema_version": 1,
    "architecture": "x86_64",
    "kernel_path": "kernel",
    "kernel_sha256": "{}",
    "rootfs_path": "rootfs",
    "rootfs_sha256": "{}",
    "agent_protocol_version": 1
  }},
  "host": "test-host",
  "memory_mib": 256,
  "vcpus": 1,
  "disk_copy": ""
}}"#,
        "d".repeat(32),
        "ab".repeat(32),
        "cd".repeat(32),
    );
    fs::write(&box_json, &payload).unwrap();
    let run_dir = box_dir.join("d".repeat(32));
    fs::create_dir_all(&run_dir).unwrap();
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let fake_bin = root.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let firecracker = fake_bin.join("firecracker");
    fs::write(&firecracker, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&firecracker, fs::Permissions::from_mode(0o755)).unwrap();
    let before_bytes = fs::read(&box_json).unwrap();
    let before_mtime = fs::metadata(&box_json).unwrap().modified().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .env("PATH", fake_bin)
        .arg("--state-dir")
        .arg(root.path())
        .args(["inspect", &id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_bytes = fs::read(&box_json).unwrap();
    let after_mtime = fs::metadata(&box_json).unwrap().modified().unwrap();
    assert_eq!(
        after_bytes, before_bytes,
        "inspect rewrote box.json contents"
    );
    assert_eq!(
        after_mtime, before_mtime,
        "inspect touched box.json modification time"
    );
    assert!(after_mtime <= SystemTime::now());
}
