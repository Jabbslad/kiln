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
fn benchmark_reports_nonoverlapping_phases_per_concurrent_launch() {
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
    let report = success(
        state.path(),
        &[
            "benchmark",
            "--image",
            &image,
            "--template",
            template["id"].as_str().unwrap(),
            "--samples",
            "2",
            "--concurrency",
            "2",
            "--allow-unsafe-development",
        ],
    );
    for mode in ["cold_boot", "template_restore"] {
        let samples = report["results"][mode]["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 2);
        for sample in samples {
            assert!(sample["error"].is_null());
            let phases = sample["phases_ms"].as_object().expect("phase timings");
            for phase in [
                "allocation_wait",
                "reservation",
                "disk_copy",
                "boot_preflight",
                "vmm_start",
                "vmm_configure",
                "guest_ready",
                "guest_initialize",
                "launch_commit",
            ] {
                assert!(phases[phase].as_f64().unwrap() > 0., "{mode}: {phase}");
            }
            assert!(phases.contains_key(if mode == "cold_boot" {
                "image_verification"
            } else {
                "snapshot_verification"
            }));
            assert!(!phases.contains_key(if mode == "cold_boot" {
                "snapshot_verification"
            } else {
                "image_verification"
            }));
            let sum: f64 = phases.values().map(|v| v.as_f64().unwrap()).sum();
            let launch = sample["launch_ms"].as_f64().unwrap();
            assert!(sum <= launch, "overlapping phases: {sum} > {launch}");
            assert!(
                sum >= launch * 0.9,
                "unattributed launch time: {sum} / {launch}"
            );
        }
    }
    assert_eq!(success(state.path(), &["list"])["boxes"], json!([]));
}

#[test]
#[ignore = "requires KVM and BOXD_TEST_IMAGE"]
fn clone_verification_releases_allocation_but_pins_snapshot_and_quota() {
    use std::{
        fs,
        io::Write,
        os::unix::fs::OpenOptionsExt,
        process::Stdio,
        time::{Duration, Instant},
    };
    let image = std::env::var("BOXD_TEST_IMAGE").expect("set BOXD_TEST_IMAGE");
    let state = tempfile::tempdir().unwrap();
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700)).unwrap();
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
    let id = template["id"].as_str().unwrap();
    // Gate checksum I/O deterministically; no timing assumption about hashing
    // speed. This deliberately damaged artifact must never reach the VMM.
    let artifact = state.path().join("snapshots").join(id).join("state.snap");
    fs::remove_file(&artifact).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&artifact)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_box"))
        .arg("--state-dir")
        .arg(state.path())
        .args(["clone", id, "--allow-unsafe-development"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut writer = loop {
        if let Ok(writer) = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&artifact)
        {
            break writer;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("clone did not start artifact verification: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let allocation_available = box_runtime::storage::lock(state.path()).is_ok();
    let records: Vec<box_runtime::runtime::BoxRecord> = fs::read_dir(state.path().join("boxes"))
        .unwrap()
        .map(|entry| {
            box_runtime::storage::read_json(&entry.unwrap().path().join("box.json")).unwrap()
        })
        .collect();
    let deletion = invoke(state.path(), &["checkpoint", "delete", id]);
    writer.write_all(b"corrupted snapshot").unwrap();
    drop(writer);
    let output = child.wait_with_output().unwrap();
    assert!(
        allocation_available,
        "checksum I/O must not hold the allocation lock"
    );
    assert_eq!(
        records.len(),
        1,
        "quota and backing-file reference must already be durable"
    );
    assert_eq!(records[0].source.as_deref(), Some(id));
    assert_eq!(records[0].memory_mib, 256);
    assert!(!deletion.status.success());
    assert!(String::from_utf8_lossy(&deletion.stderr).contains("referenced"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("checksum mismatch"));
    let failed: Value = box_runtime::storage::read_json(
        &state
            .path()
            .join("boxes")
            .join(&records[0].id)
            .join("box.json"),
    )
    .unwrap();
    assert_eq!(failed["state"], "failed");
    assert!(failed["process"].is_null());
    assert!(
        failed["last_error"]
            .as_str()
            .unwrap()
            .contains("checksum mismatch")
    );
    assert!(success(state.path(), &["inspect", &records[0].id])["process"].is_null());
    success(state.path(), &["delete", &records[0].id]);
    success(state.path(), &["checkpoint", "delete", id]);
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
