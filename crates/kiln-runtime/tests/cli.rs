use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, process::Command};

#[test]
fn rejects_unsafe_profiles_and_path_traversal_without_launching() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    for (arguments, message) in [
        (
            vec!["create", "--image", "nonexistent"],
            "--allow-unsafe-development",
        ),
        (
            vec![
                "create",
                "--image",
                "nonexistent",
                "--profile",
                "isolated",
                "--allow-unsafe-development",
            ],
            "refusing unjailed fallback",
        ),
        (vec!["inspect", "../../outside"], "invalid box ID"),
        (
            vec!["checkpoint", "delete", "../outside"],
            "invalid snapshot ID",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
            .arg("--state-dir")
            .arg(root.path())
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(message)
        );
    }
    assert_eq!(
        std::fs::read_dir(root.path().join("boxes"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn rejects_shared_or_symlink_state_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .arg("--state-dir")
        .arg(root.path())
        .arg("list")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("accessible by group"));
    let link = root.path().join("link");
    std::os::unix::fs::symlink(root.path(), &link).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .arg("--state-dir")
        .arg(&link)
        .arg("list")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a directory"));
}

#[test]
fn isolated_configuration_never_falls_back_to_development() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .arg("--state-dir")
        .arg(root.path())
        .args(["--isolation-config", "/nonexistent/kiln.json", "list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!root.path().join("boxes").exists());

    // A persisted policy is mandatory, even for commands without --profile.
    std::fs::write(root.path().join("isolation.json"), b"{}").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .arg("--state-dir")
        .arg(root.path())
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn broken_isolation_policy_link_does_not_reopen_as_development() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink("missing-policy", root.path().join("isolation.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .arg("--state-dir")
        .arg(root.path())
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn isolated_doctor_validates_policy_before_executing_path_binaries() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("firecracker");
    let marker = root.path().join("executed");
    std::fs::write(
        &binary,
        format!("#!/bin/sh\nprintf unsafe > {}\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln-runtime"))
        .env("PATH", root.path())
        .args(["doctor", "--isolation-config", "/nonexistent/kiln.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists(), "unvalidated PATH binary was executed");
}
