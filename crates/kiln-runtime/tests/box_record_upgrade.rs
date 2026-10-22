use kiln_runtime::runtime::{BoxRecord, Runtime};
use std::os::unix::fs::PermissionsExt;

const LEGACY_BOX_JSON: &str = r#"{
  "schema_version": 1,
  "id": "0123456789abcdef0123456789abcdef",
  "name": "jamie",
  "state": "stopped",
  "run": "0123456789abcdef0123456789abcde0",
  "process": null,
  "image": {
    "schema_version": 1,
    "architecture": "x86_64",
    "kernel_path": "/kernel",
    "kernel_sha256": "abababababababababababababababababababababababababababababababab",
    "rootfs_path": "/rootfs",
    "rootfs_sha256": "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
    "agent_protocol_version": 1,
    "boot_mode": "init"
  },
  "host": "fixture-host",
  "memory_mib": 256,
  "vcpus": 1,
  "source": null,
  "operation": null,
  "last_error": null,
  "disk_copy": "copy",
  "disk_layer": null,
  "jail": null,
  "network": null,
  "last_launch": null
}"#;

#[test]
fn pre_change_box_json_gets_tenant_owner_on_read() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = Runtime::open(root.path())
        .unwrap()
        .with_tenant_owner("acct-jamie".into());
    let id = "0123456789abcdef0123456789abcdef";
    let directory = root.path().join("boxes").join(id);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(directory.join("box.json"), LEGACY_BOX_JSON.as_bytes()).unwrap();

    let record = runtime.read_sync(id).unwrap();
    assert_eq!(record.owner, "acct-jamie");
    assert_eq!(record.name, "jamie");
}

#[test]
fn explicit_owner_is_preserved() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = Runtime::open(root.path())
        .unwrap()
        .with_tenant_owner("acct-jamie".into());
    let mut record: BoxRecord = serde_json::from_str(LEGACY_BOX_JSON).unwrap();
    record.owner = "acct-other".into();
    let id = record.id.clone();
    let directory = root.path().join("boxes").join(&id);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        directory.join("box.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();

    let loaded = runtime.read_sync(&id).unwrap();
    assert_eq!(loaded.owner, "acct-other");
}
