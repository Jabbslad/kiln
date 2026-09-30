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
        let output = Command::new(env!("CARGO_BIN_EXE_box"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_box"))
        .arg("--state-dir")
        .arg(root.path())
        .arg("list")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("accessible by group"));
    let link = root.path().join("link");
    std::os::unix::fs::symlink(root.path(), &link).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_box"))
        .arg("--state-dir")
        .arg(&link)
        .arg("list")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a directory"));
}
