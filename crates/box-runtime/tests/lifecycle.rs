use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::{
    path::Path,
    process::{Command, Output},
};

fn invoke(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_box"))
        .arg("--state-dir")
        .arg(state)
        .args(args)
        .output()
        .unwrap()
}

fn success(state: &Path, args: &[&str]) -> Value {
    let output = invoke(state, args);
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Cleanup<'a>(&'a Path);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for entry in std::fs::read_dir(self.0.join("boxes"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if let Ok(record) = box_runtime::storage::read_json::<box_runtime::runtime::BoxRecord>(
                &entry.path().join("box.json"),
            ) {
                if std::thread::panicking() {
                    let log =
                        std::fs::read_to_string(entry.path().join(&record.run).join("console.log"))
                            .unwrap_or_default();
                    eprintln!("guest log: {}", &log[log.len().saturating_sub(8000)..]);
                }
                if let Some(process) = record.process {
                    let _ = box_runtime::process::terminate(&process);
                }
            }
        }
    }
}

#[test]
#[ignore = "requires KVM and BOXD_TEST_IMAGE"]
fn real_lifecycle_persists_disk_and_restores_memory() {
    let image =
        std::env::var("BOXD_TEST_IMAGE").expect("set BOXD_TEST_IMAGE to a built image.json");
    let state = tempfile::tempdir().unwrap();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Cleanup(state.path());
    let created = success(
        state.path(),
        &[
            "create",
            "--image",
            &image,
            "--name",
            "test",
            "--allow-unsafe-development",
        ],
    );
    let id = created["id"].as_str().unwrap();
    let result = success(
        state.path(),
        &[
            "exec",
            id,
            "--",
            "/bin/sh",
            "-c",
            "printf before > /workspace/marker; printf alpha; printf beta >&2",
        ],
    );
    assert_eq!(result["stdout"], "alpha");
    assert_eq!(result["stderr"], "beta");
    let failure = invoke(state.path(), &["exec", id, "--", "/bin/sh", "-c", "exit 7"]);
    assert_eq!(failure.status.code(), Some(7));
    success(state.path(), &["stop", id]);
    success(state.path(), &["start", id]);
    assert_eq!(
        success(
            state.path(),
            &["exec", id, "--", "/bin/cat", "/workspace/marker"]
        )["stdout"],
        "before"
    );
    let boot = success(
        state.path(),
        &[
            "exec",
            id,
            "--",
            "/bin/cat",
            "/proc/sys/kernel/random/boot_id",
        ],
    )["stdout"]
        .clone();
    success(state.path(), &["pause", id]);
    assert!(
        !invoke(state.path(), &["exec", id, "--", "/bin/true"])
            .status
            .success()
    );
    success(state.path(), &["resume", id]);
    assert_eq!(
        success(
            state.path(),
            &["exec", id, "--", "/bin/printf", "after-resume"]
        )["stdout"],
        "after-resume"
    );
    let marker = format!("MEMORY_MARKER=only-in-process-{id}");
    success(
        state.path(),
        &[
            "exec",
            id,
            "--env",
            &marker,
            "--",
            "/bin/sh",
            "-c",
            "/bin/sleep 300 </dev/null >/dev/null 2>&1 & printf %s $! > /workspace/memory-pid",
        ],
    );
    let memory_command = "cat /proc/$(cat /workspace/memory-pid)/environ";
    assert!(
        success(
            state.path(),
            &["exec", id, "--", "/bin/sh", "-c", memory_command]
        )["stdout"]
            .as_str()
            .unwrap()
            .contains(&marker)
    );
    let checkpoint = success(state.path(), &["checkpoint", "save", id]);
    success(
        state.path(),
        &[
            "exec",
            id,
            "--",
            "/bin/sh",
            "-c",
            "kill $(cat /workspace/memory-pid); sleep 0.05",
        ],
    );
    let killed = invoke(
        state.path(),
        &["exec", id, "--", "/bin/sh", "-c", memory_command],
    );
    assert!(!String::from_utf8_lossy(&killed.stdout).contains(&marker));
    assert!(
        !invoke(
            state.path(),
            &[
                "clone",
                checkpoint["id"].as_str().unwrap(),
                "--allow-unsafe-development"
            ]
        )
        .status
        .success()
    );
    assert!(
        !invoke(
            state.path(),
            &[
                "checkpoint",
                "restore",
                id,
                checkpoint["id"].as_str().unwrap()
            ]
        )
        .status
        .success()
    );
    success(
        state.path(),
        &[
            "exec",
            id,
            "--",
            "/bin/sh",
            "-c",
            "printf after > /workspace/marker",
        ],
    );
    success(
        state.path(),
        &[
            "checkpoint",
            "restore",
            id,
            checkpoint["id"].as_str().unwrap(),
            "--acknowledge-external-state-replay",
        ],
    );
    assert_eq!(
        success(
            state.path(),
            &["exec", id, "--", "/bin/cat", "/workspace/marker"]
        )["stdout"],
        "before"
    );
    assert!(
        success(
            state.path(),
            &["exec", id, "--", "/bin/sh", "-c", memory_command]
        )["stdout"]
            .as_str()
            .unwrap()
            .contains(&marker)
    );
    assert_eq!(
        success(
            state.path(),
            &[
                "exec",
                id,
                "--",
                "/bin/cat",
                "/proc/sys/kernel/random/boot_id"
            ]
        )["stdout"],
        boot
    );
    success(state.path(), &["pause", id]);
    success(state.path(), &["checkpoint", "save", id]);
    assert_eq!(success(state.path(), &["inspect", id])["state"], "paused");
    success(state.path(), &["resume", id]);
    // A stale checksum must be rejected before stopping the running source.
    let manifest = state
        .path()
        .join("snapshots")
        .join(checkpoint["id"].as_str().unwrap())
        .join("snapshot.json");
    let mut corrupted = checkpoint.clone();
    corrupted["hashes"]["state.snap"] = json!("not-the-real-hash");
    box_runtime::storage::atomic_json(&manifest, &corrupted).unwrap();
    assert!(
        !invoke(
            state.path(),
            &[
                "checkpoint",
                "restore",
                id,
                checkpoint["id"].as_str().unwrap(),
                "--acknowledge-external-state-replay"
            ]
        )
        .status
        .success()
    );
    assert_eq!(success(state.path(), &["inspect", id])["state"], "running");
    success(state.path(), &["delete", id]);
    assert_eq!(success(state.path(), &["list"])["boxes"], json!([]));
}

#[test]
#[ignore = "requires KVM and BOXD_TEST_IMAGE"]
fn prepared_clones_have_private_identity_disks_and_lifetimes() {
    let image = std::env::var("BOXD_TEST_IMAGE").expect("set BOXD_TEST_IMAGE");
    let state = tempfile::tempdir().unwrap();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Cleanup(state.path());
    let template = success(
        state.path(),
        &[
            "template",
            "build",
            "--image",
            &image,
            "--allow-unsafe-development",
        ],
    );
    assert_eq!(success(state.path(), &["list"])["boxes"], json!([]));
    let template_id = template["id"].as_str().unwrap();
    let a = success(
        state.path(),
        &[
            "clone",
            template_id,
            "--name",
            "alpha",
            "--allow-unsafe-development",
        ],
    );
    let b = success(
        state.path(),
        &[
            "clone",
            template_id,
            "--name",
            "beta",
            "--allow-unsafe-development",
        ],
    );
    let aid = a["id"].as_str().unwrap();
    let bid = b["id"].as_str().unwrap();
    assert_ne!(aid, bid);
    assert_eq!(a["source"], template["id"]);
    assert_eq!(b["source"], template["id"]);
    assert!(
        !invoke(state.path(), &["checkpoint", "delete", template_id])
            .status
            .success()
    );
    for (id, marker) in [(aid, "alpha-17"), (bid, "beta-93")] {
        success(
            state.path(),
            &[
                "exec",
                id,
                "--",
                "/bin/sh",
                "-c",
                &format!("printf {marker} > /workspace/value"),
            ],
        );
        assert_eq!(
            success(
                state.path(),
                &["exec", id, "--", "/bin/cat", "/etc/machine-id"]
            )["stdout"],
            format!("{id}\n")
        );
        assert_eq!(
            success(state.path(), &["exec", id, "--", "/bin/hostname"])["stdout"],
            format!("box-{}\n", &id[..12])
        );
    }
    let arandom = success(
        state.path(),
        &[
            "exec",
            aid,
            "--",
            "/bin/cat",
            "/proc/sys/kernel/random/uuid",
        ],
    )["stdout"]
        .clone();
    let brandom = success(
        state.path(),
        &[
            "exec",
            bid,
            "--",
            "/bin/cat",
            "/proc/sys/kernel/random/uuid",
        ],
    )["stdout"]
        .clone();
    assert_ne!(arandom, brandom);
    let aboot = success(
        state.path(),
        &[
            "exec",
            aid,
            "--",
            "/bin/cat",
            "/proc/sys/kernel/random/boot_id",
        ],
    )["stdout"]
        .clone();
    let bboot = success(
        state.path(),
        &[
            "exec",
            bid,
            "--",
            "/bin/cat",
            "/proc/sys/kernel/random/boot_id",
        ],
    )["stdout"]
        .clone();
    assert_eq!(
        aboot, bboot,
        "clones must restore the prepared kernel, not silently cold boot"
    );
    assert_eq!(
        success(
            state.path(),
            &["exec", aid, "--", "/bin/cat", "/workspace/value"]
        )["stdout"],
        "alpha-17"
    );
    success(state.path(), &["delete", aid]);
    assert_eq!(
        success(
            state.path(),
            &["exec", bid, "--", "/bin/cat", "/workspace/value"]
        )["stdout"],
        "beta-93"
    );
    success(state.path(), &["delete", bid]);
    success(state.path(), &["checkpoint", "delete", template_id]);
    assert_eq!(
        success(state.path(), &["template", "list"])["templates"],
        json!([])
    );
}

#[test]
#[cfg(feature = "fault-injection")]
#[ignore = "requires KVM and BOXD_TEST_IMAGE; deliberately crashes disposable CLI processes"]
fn manager_crashes_do_not_duplicate_vmm_or_publish_partial_snapshots() {
    let image = std::env::var("BOXD_TEST_IMAGE").expect("set BOXD_TEST_IMAGE");
    let state = tempfile::tempdir().unwrap();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Cleanup(state.path());
    for point in ["before-spawn", "after-spawn", "after-process-record"] {
        let output = Command::new(env!("CARGO_BIN_EXE_box"))
            .arg("--state-dir")
            .arg(state.path())
            .args(["create", "--image", &image, "--allow-unsafe-development"])
            .env("BOXD_FAILPOINT", point)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "failpoint {point}: {:?}",
            output
        );
        let records = success(state.path(), &["list"]);
        let records = records["boxes"].as_array().unwrap();
        assert_eq!(records.len(), 1);
        let id = records[0]["id"].as_str().unwrap();
        assert_eq!(records[0]["state"], "stopped");
        assert!(records[0]["process"].is_null());
        let run = state
            .path()
            .join("boxes")
            .join(id)
            .join(records[0]["run"].as_str().unwrap());
        assert!(
            box_runtime::process::find(
                &run,
                &box_runtime::host::executable("firecracker").unwrap()
            )
            .unwrap()
            .is_none()
        );
        success(state.path(), &["start", id]);
        assert_eq!(
            success(
                state.path(),
                &["exec", id, "--", "/bin/printf", "recovered"]
            )["stdout"],
            "recovered"
        );
        success(state.path(), &["delete", id]);
    }
    let created = success(
        state.path(),
        &["create", "--image", &image, "--allow-unsafe-development"],
    );
    let id = created["id"].as_str().unwrap();
    for (index, point) in ["after-pause", "after-snapshot-publication"]
        .iter()
        .enumerate()
    {
        let output = Command::new(env!("CARGO_BIN_EXE_box"))
            .arg("--state-dir")
            .arg(state.path())
            .args(["checkpoint", "save", id])
            .env("BOXD_FAILPOINT", point)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(86));
        let recovered = success(state.path(), &["inspect", id]);
        assert_eq!(recovered["state"], "running");
        assert_eq!(recovered["process"], created["process"]);
        let published = std::fs::read_dir(state.path().join("snapshots"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("snapshot.json").exists())
            .count();
        assert_eq!(published, index);
        assert_eq!(
            success(state.path(), &["exec", id, "--", "/bin/printf", "resumed"])["stdout"],
            "resumed"
        );
    }
    success(state.path(), &["delete", id]);
}

#[test]
#[ignore = "requires KVM and BOXD_TEST_IMAGE"]
fn simultaneous_starts_cannot_create_two_disk_writers() {
    let image = std::env::var("BOXD_TEST_IMAGE").expect("set BOXD_TEST_IMAGE");
    let state = tempfile::tempdir().unwrap();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Cleanup(state.path());
    let created = success(
        state.path(),
        &["create", "--image", &image, "--allow-unsafe-development"],
    );
    let id = created["id"].as_str().unwrap();
    success(state.path(), &["stop", id]);
    let outcomes = std::thread::scope(|scope| {
        let a = scope.spawn(|| invoke(state.path(), &["start", id]));
        let b = scope.spawn(|| invoke(state.path(), &["start", id]));
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(outcomes.iter().filter(|r| r.status.success()).count(), 1);
    let record = success(state.path(), &["inspect", id]);
    assert_eq!(record["state"], "running");
    let run = state
        .path()
        .join("boxes")
        .join(id)
        .join(record["run"].as_str().unwrap());
    assert!(
        box_runtime::process::find(&run, &box_runtime::host::executable("firecracker").unwrap())
            .unwrap()
            .is_some()
    );
    std::fs::rename(run.join("api.sock"), run.join("api.unreachable")).unwrap();
    success(state.path(), &["stop", id, "--force"]);
    assert_eq!(success(state.path(), &["inspect", id])["state"], "stopped");
    success(state.path(), &["delete", id]);
}
