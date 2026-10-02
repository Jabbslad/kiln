use serde_json::{Value, json};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn command(state: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(
        std::env::var_os("KILN_TEST_RUNTIME")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_kiln-runtime").into()),
    );
    command.arg("--state-dir").arg(state);
    if let Ok(config) = std::env::var("KILN_TEST_ISOLATION_CONFIG") {
        if !state.join("isolation.json").exists() {
            command.arg("--isolation-config").arg(config);
        }
        command.args(
            args.iter()
                .filter(|arg| **arg != "--allow-unsafe-development"),
        );
        if matches!(args.first(), Some(&"create" | &"clone" | &"benchmark"))
            || args.starts_with(&["template", "build"])
        {
            command.args(["--profile", "isolated"]);
        }
    } else {
        command.args(args);
    }
    command
}

struct StateDirectory(Option<tempfile::TempDir>);

impl StateDirectory {
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}

impl Drop for StateDirectory {
    fn drop(&mut self) {
        let retain = match std::fs::read_dir(self.path().join("boxes")) {
            Ok(mut entries) => entries.next().is_some(),
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        };
        if retain {
            let path = self.0.take().unwrap().keep();
            eprintln!(
                "Retained unfinished VM state for diagnosis: {}",
                path.display()
            );
        }
    }
}

fn state_directory() -> StateDirectory {
    let directory = if std::env::var_os("KILN_TEST_ISOLATION_CONFIG").is_some() {
        kiln_runtime::isolation::require_root().unwrap();
        let parent = std::env::var("KILN_TEST_STATE_PARENT")
            .expect("isolated tests need a root-owned KILN_TEST_STATE_PARENT; run serially");
        kiln_runtime::isolation::trusted_path(Path::new(&parent)).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    StateDirectory(Some(directory))
}

fn run_directory(state: &Path, record: &Value) -> PathBuf {
    let generation = record["run"].as_str().unwrap();
    let root = state
        .join("boxes")
        .join(record["id"].as_str().unwrap())
        .join(generation);
    if record["jail"].is_object() {
        root.join("firecracker").join(generation).join("root")
    } else {
        root
    }
}

fn invoke(state: &Path, args: &[&str]) -> Output {
    command(state, args).output().unwrap()
}

fn success(state: &Path, args: &[&str]) -> Value {
    let output = invoke(state, args);
    assert!(
        output.status.success(),
        "{:?}: {}\nstdout: {}\nstderr: {}",
        args,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    if value["hashes"].is_object() && std::env::var_os("KILN_TEST_REQUIRE_VERITY").is_some() {
        for name in ["disk.ext4", "state.snap", "memory.snap"] {
            assert_eq!(value["seals"][name]["sha256"], value["hashes"][name]);
            assert!(
                value["seals"][name]["digest"].is_string(),
                "missing seal for {name}: {value}"
            );
        }
    }
    if value["process"].is_object() && value["jail"].is_object() {
        let pid = value["process"]["pid"].as_u64().unwrap();
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let uid = value["jail"]["uid"].as_u64().unwrap();
        assert_ne!(uid, 0);
        assert!(
            status
                .lines()
                .any(|line| line == format!("Uid:\t{uid}\t{uid}\t{uid}\t{uid}"))
        );
        for field in ["NoNewPrivs:\t1", "Seccomp:\t2", "CapEff:\t0000000000000000"] {
            assert!(status.lines().any(|line| line == field), "missing {field}");
        }
        let run = run_directory(state, &value);
        let observed_root = std::fs::metadata(format!("/proc/{pid}/root")).unwrap();
        let expected_root = std::fs::metadata(&run).unwrap();
        assert_eq!(
            (observed_root.dev(), observed_root.ino()),
            (expected_root.dev(), expected_root.ino())
        );
        assert_eq!(
            std::fs::metadata(run.join("disk.ext4")).unwrap().uid(),
            uid as u32
        );
        let disk = std::fs::metadata(run.join("disk.ext4")).unwrap();
        assert_eq!(
            disk.file_type().is_block_device(),
            value["disk_layer"].is_string()
        );
        assert_eq!(disk.mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(state.join("boxes")).unwrap().mode() & 0o777,
            0o700
        );
        let config: kiln_runtime::isolation::Config =
            kiln_runtime::storage::read_json(&state.join("isolation.json")).unwrap();
        let group = Path::new("/sys/fs/cgroup")
            .join(&config.cgroup_parent)
            .join(value["run"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
                .unwrap()
                .trim(),
            format!(
                "0::/{}/{}",
                config.cgroup_parent,
                value["run"].as_str().unwrap()
            )
        );
        for (file, expected) in [
            ("pids.max", "64".to_owned()),
            ("memory.swap.max", "0".to_owned()),
            (
                "cpu.max",
                format!("{} 100000", value["vcpus"].as_u64().unwrap() * 100000),
            ),
            (
                "memory.max",
                ((value["memory_mib"].as_u64().unwrap() * 2 + 128) * 1048576).to_string(),
            ),
        ] {
            assert_eq!(
                std::fs::read_to_string(group.join(file)).unwrap().trim(),
                expected
            );
        }
    }
    value
}

struct Cleanup<'a>(&'a Path);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for entry in std::fs::read_dir(self.0.join("boxes"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if let Ok(record) = kiln_runtime::storage::read_json::<kiln_runtime::runtime::BoxRecord>(
                &entry.path().join("box.json"),
            ) {
                if std::thread::panicking() {
                    let log =
                        std::fs::read_to_string(entry.path().join(&record.run).join("console.log"))
                            .unwrap_or_default();
                    eprintln!("guest log: {}", &log[log.len().saturating_sub(8000)..]);
                }
                let output = invoke(self.0, &["delete", &record.id]);
                if !output.status.success() {
                    eprintln!(
                        "cleanup failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    if let Some(process) = record.process {
                        let _ = kiln_runtime::process::terminate(&process);
                    }
                }
            }
        }
    }
}

#[test]
fn failed_cleanup_preserves_state_but_successful_cleanup_removes_it() {
    let parent = tempfile::tempdir().unwrap();
    let state = StateDirectory(Some(tempfile::tempdir_in(parent.path()).unwrap()));
    let retained = state.path().to_owned();
    std::fs::create_dir(retained.join("boxes")).unwrap();
    std::fs::create_dir(retained.join("boxes/unfinished")).unwrap();
    drop(state);
    assert!(retained.exists());
    let state = StateDirectory(Some(tempfile::tempdir_in(parent.path()).unwrap()));
    let removed = state.path().to_owned();
    std::fs::create_dir(removed.join("boxes")).unwrap();
    drop(state);
    assert!(!removed.exists());
}

#[test]
#[ignore = "requires KVM and KILN_TEST_IMAGE"]
fn real_lifecycle_persists_disk_and_restores_memory() {
    let image =
        std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE to a built image.json");
    let state = state_directory();
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
    if created["jail"].is_object() {
        assert_eq!(
            success(
                state.path(),
                &[
                    "exec",
                    id,
                    "--",
                    "/bin/sh",
                    "-c",
                    "ls /sys/class/net; test ! -S /api.sock"
                ]
            )["stdout"],
            "lo\n"
        );
    }
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
    kiln_runtime::storage::atomic_json(&manifest, &corrupted).unwrap();
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
#[ignore = "requires KVM and KILN_TEST_IMAGE"]
fn prepared_clones_have_private_identity_disks_and_lifetimes() {
    let image = std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE");
    let state = state_directory();
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
    let expected_id = |id: &str| {
        if a["image"]["boot_mode"] == "systemd_warm_shared" {
            "11111111111111111111111111111111".to_owned()
        } else {
            id.to_owned()
        }
    };
    if a["jail"].is_object() {
        assert_ne!(a["jail"]["uid"], b["jail"]["uid"]);
        assert_ne!(a["jail"]["gid"], b["jail"]["gid"]);
        let a_run = run_directory(state.path(), &a);
        let b_run = run_directory(state.path(), &b);
        for name in ["disk.ext4", "state.snap", "memory.snap"] {
            let source = state.path().join("snapshots").join(template_id).join(name);
            let original = std::fs::metadata(&source).unwrap();
            let first = std::fs::metadata(a_run.join(name)).unwrap();
            let second = std::fs::metadata(b_run.join(name)).unwrap();
            if name != "disk.ext4" && template["seals"][name].is_object() {
                assert_eq!((first.dev(), first.ino()), (original.dev(), original.ino()));
                assert_eq!(
                    (second.dev(), second.ino()),
                    (original.dev(), original.ino())
                );
                assert_eq!(first.uid(), 0);
                assert!(
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(a_run.join(name))
                        .is_err()
                );
            } else {
                assert_ne!((first.dev(), first.ino()), (second.dev(), second.ino()));
                assert_ne!((first.dev(), first.ino()), (original.dev(), original.ino()));
            }
        }
        assert_eq!(
            std::fs::metadata(a_run.join("memory.snap")).unwrap().mode() & 0o777,
            0o444
        );
    }
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
            format!("{}\n", expected_id(id))
        );
        assert_eq!(
            success(state.path(), &["exec", id, "--", "/bin/hostname"])["stdout"],
            format!("box-{}\n", &id[..12])
        );
        if matches!(
            a["image"]["boot_mode"].as_str(),
            Some("systemd" | "systemd_warm" | "systemd_warm_shared")
        ) {
            let script = "set -e; test \"$(cat /proc/1/comm)\" = systemd; . /etc/os-release; test \"$ID:$VERSION_ID\" = ubuntu:24.04; systemd-run --quiet --wait --pipe /bin/cat /etc/machine-id; python3 -c 'print(sum([17, 93, -8]))'";
            let result = success(
                state.path(),
                &[
                    "exec",
                    id,
                    "--timeout-ms",
                    "10000",
                    "--",
                    "/bin/sh",
                    "-c",
                    script,
                ],
            );
            assert_eq!(result["exit_code"], 0, "{result}");
            assert_eq!(result["stdout"], format!("{}\n102\n", expected_id(id)));
            let result = success(
                state.path(),
                &[
                    "exec",
                    id,
                    "--",
                    "/usr/bin/busctl",
                    "--system",
                    "call",
                    "org.freedesktop.systemd1",
                    "/org/freedesktop/systemd1",
                    "org.freedesktop.DBus.Peer",
                    "GetMachineId",
                ],
            );
            assert_eq!(result["exit_code"], 0, "{result}");
            assert_eq!(
                result["stdout"],
                format!("s \"{}\"\n", expected_id(id)),
                "systemd identity must match the explicit image policy"
            );
        }
    }
    if matches!(
        a["image"]["boot_mode"].as_str(),
        Some("systemd" | "systemd_warm" | "systemd_warm_shared")
    ) {
        let script = "set -e; busctl call org.freedesktop.DBus / org.freedesktop.DBus GetId; systemctl show --value -p InvocationID dbus.service systemd-journald.service systemd-udevd.service kiln-guest.service";
        let mut identities = Vec::new();
        for id in [aid, bid] {
            let result = success(state.path(), &["exec", id, "--", "/bin/sh", "-c", script]);
            assert_eq!(result["exit_code"], 0, "{result}");
            let lines: Vec<String> = result["stdout"]
                .as_str()
                .unwrap()
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            assert_eq!(lines.len(), 5, "{result}");
            for invocation in &lines[1..] {
                assert_eq!(invocation.len(), 32);
                assert_ne!(invocation, &"0".repeat(32));
            }
            identities.push(lines);
            let journal = success(
                state.path(),
                &[
                    "exec",
                    id,
                    "--",
                    "/bin/sh",
                    "-c",
                    "set -e; logger -t kiln-identity-test ready; journalctl --sync; journalctl --no-pager -t kiln-identity-test -n 1 -o json",
                ],
            );
            assert_eq!(journal["exit_code"], 0, "{journal}");
            let entry: Value = serde_json::from_str(journal["stdout"].as_str().unwrap()).unwrap();
            assert_eq!(entry["_MACHINE_ID"], expected_id(id));
            assert_eq!(entry["_HOSTNAME"], format!("box-{}", &id[..12]));
        }
        for (first, second) in identities[0].iter().zip(&identities[1]) {
            assert_ne!(
                first, second,
                "daemon/bus identity must not be inherited from template"
            );
        }
        let args = [
            "exec",
            aid,
            "--",
            "/bin/systemctl",
            "show",
            "--property=MainPID",
            "--value",
            "kiln-guest.service",
        ];
        let before = success(state.path(), &args)["stdout"].clone();
        let restart = success(
            state.path(),
            &[
                "exec",
                aid,
                "--",
                "/usr/bin/systemd-run",
                "--quiet",
                "--on-active=1s",
                "/bin/systemctl",
                "restart",
                "kiln-guest.service",
            ],
        );
        assert_eq!(restart["exit_code"], 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let output = invoke(state.path(), &args);
            if output.status.success() {
                let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                if value["exit_code"] == 0 && value["stdout"] != before && value["stdout"] != "0\n"
                {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "systemd agent did not restart ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(
            success(
                state.path(),
                &["exec", aid, "--", "/bin/cat", "/etc/machine-id"]
            )["stdout"],
            format!("{}\n", expected_id(aid))
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
    if matches!(
        a["image"]["boot_mode"].as_str(),
        Some("systemd_warm" | "systemd_warm_shared")
    ) {
        // Admission is for new templates, not for restarting user-modified boxes.
        let install = "set -e; mkdir -p /etc/credstore; printf synthetic-custom-93 > /etc/credstore/boot-probe; printf '%s\\n' '[Unit]' 'After=basic.target' 'Before=kiln-guest.service' '[Service]' 'Type=oneshot' 'RemainAfterExit=yes' 'LoadCredential=boot-probe:/etc/credstore/boot-probe' 'ExecStart=/bin/sh -c \"cat ${CREDENTIALS_DIRECTORY}/boot-probe > /workspace/custom-started\"' '[Install]' 'WantedBy=kiln.target' > /etc/systemd/system/custom-workload.service; systemctl enable custom-workload.service";
        assert_eq!(
            success(state.path(), &["exec", aid, "--", "/bin/sh", "-c", install])["exit_code"],
            0
        );
    }
    success(state.path(), &["stop", aid]);
    let restarted = success(state.path(), &["start", aid]);
    assert_eq!(restarted["disk_layer"], a["disk_layer"]);
    assert_eq!(
        success(
            state.path(),
            &["exec", aid, "--", "/bin/cat", "/etc/machine-id"]
        )["stdout"],
        format!("{}\n", expected_id(aid))
    );
    if matches!(
        a["image"]["boot_mode"].as_str(),
        Some("systemd_warm" | "systemd_warm_shared")
    ) {
        let result = success(
            state.path(),
            &["exec", aid, "--", "/bin/cat", "/workspace/custom-started"],
        );
        assert_eq!(result["exit_code"], 0, "{result}");
        assert_eq!(result["stdout"], "synthetic-custom-93");
    }
    assert_eq!(
        success(
            state.path(),
            &["exec", aid, "--", "/bin/cat", "/workspace/value"]
        )["stdout"],
        "alpha-17"
    );
    // The retained layer must remain writable across jail generations.
    if a["disk_layer"].is_string() {
        assert!(
            !invoke(state.path(), &["checkpoint", "delete", template_id])
                .status
                .success()
        );
    }
    success(state.path(), &["delete", aid]);
    assert_eq!(
        success(
            state.path(),
            &["exec", bid, "--", "/bin/cat", "/workspace/value"]
        )["stdout"],
        "beta-93"
    );
    if b["disk_layer"].is_string() {
        success(state.path(), &["stop", bid]);
        success(state.path(), &["start", bid]);
        success(state.path(), &["stop", bid]);
        // A cold-started generation no longer maps template memory. Model a
        // cleared memory reference; disk ownership alone must pin the base.
        let mut restarted = success(state.path(), &["inspect", bid]);
        restarted["source"] = Value::Null;
        kiln_runtime::storage::atomic_json(
            &state.path().join("boxes").join(bid).join("box.json"),
            &restarted,
        )
        .unwrap();
        assert!(
            !invoke(state.path(), &["checkpoint", "delete", template_id])
                .status
                .success()
        );
    }
    success(state.path(), &["delete", bid]);
    if let Ok(config) = std::env::var("KILN_TEST_ISOLATION_CONFIG") {
        let config: kiln_runtime::isolation::Config =
            kiln_runtime::storage::read_json(Path::new(&config)).unwrap();
        if config.disk_backend == kiln_runtime::isolation::DiskBackend::Snapshot {
            assert_eq!(a["disk_copy"], "Snapshot");
        }
    }
    success(state.path(), &["checkpoint", "delete", template_id]);
    assert_eq!(
        success(state.path(), &["template", "list"])["templates"],
        json!([])
    );
}

#[test]
#[cfg(feature = "fault-injection")]
#[ignore = "requires KVM and KILN_TEST_IMAGE; deliberately crashes disposable CLI processes"]
fn manager_crashes_do_not_duplicate_vmm_or_publish_partial_snapshots() {
    let image = std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE");
    let state = state_directory();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Cleanup(state.path());
    for point in ["before-spawn", "after-spawn", "after-process-record"] {
        let output = command(
            state.path(),
            &["create", "--image", &image, "--allow-unsafe-development"],
        )
        .env("KILN_FAILPOINT", point)
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
        let run = run_directory(state.path(), &records[0]);
        let executable = if records[0]["jail"].is_object() {
            run.join("firecracker")
        } else {
            kiln_runtime::host::executable("firecracker").unwrap()
        };
        assert!(
            kiln_runtime::process::find(&run, &[&executable])
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
        let output = command(state.path(), &["checkpoint", "save", id])
            .env("KILN_FAILPOINT", point)
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
    let template_id = template["id"].as_str().unwrap();
    let policy = state.path().join("isolation.json");
    if policy.exists()
        && kiln_runtime::storage::read_json::<kiln_runtime::isolation::Config>(&policy)
            .unwrap()
            .disk_backend
            == kiln_runtime::isolation::DiskBackend::Snapshot
    {
        for point in ["after-layer-publication", "after-disk-mapping"] {
            let output = command(
                state.path(),
                &["clone", template_id, "--allow-unsafe-development"],
            )
            .env("KILN_FAILPOINT", point)
            .output()
            .unwrap();
            assert_eq!(output.status.code(), Some(86), "{point}: {output:?}");
            let records = success(state.path(), &["list"]);
            let record = &records["boxes"][0];
            let id = record["id"].as_str().unwrap();
            assert_eq!(record["state"], "stopped");
            assert!(record["disk_layer"].is_string());
            assert!(
                !invoke(state.path(), &["checkpoint", "delete", template_id])
                    .status
                    .success()
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
    }
    success(state.path(), &["checkpoint", "delete", template_id]);
}

#[test]
#[ignore = "requires KVM and KILN_TEST_IMAGE"]
fn benchmark_reports_nonoverlapping_phases_per_concurrent_launch() {
    let image = std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE");
    let state = state_directory();
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
    assert_eq!(
        report["profile"],
        if template["isolated"] == true {
            "isolated"
        } else {
            "development"
        }
    );
    for name in ["disk.ext4", "state.snap", "memory.snap"] {
        assert_eq!(
            report["template_verification"][name],
            if template["seals"][name].is_object() {
                "fs_verity"
            } else {
                "sha256"
            }
        );
    }
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
#[ignore = "requires KVM and KILN_TEST_IMAGE"]
fn clone_verification_releases_allocation_but_pins_snapshot_and_quota() {
    use std::{
        fs,
        io::Write,
        os::unix::fs::OpenOptionsExt,
        process::Stdio,
        time::{Duration, Instant},
    };
    let image = std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE");
    let state = state_directory();
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
    // Exercise the legacy full-scan path even on a verity-capable filesystem.
    // A claimed seal on a FIFO instead fails immediately (covered by unit tests).
    let mut legacy = template.clone();
    legacy.as_object_mut().unwrap().remove("seals");
    kiln_runtime::storage::atomic_json(
        &state
            .path()
            .join("snapshots")
            .join(id)
            .join("snapshot.json"),
        &legacy,
    )
    .unwrap();
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
    let mut child = command(state.path(), &["clone", id, "--allow-unsafe-development"])
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
    let allocation_available = kiln_runtime::storage::lock(state.path()).is_ok();
    let records: Vec<kiln_runtime::runtime::BoxRecord> = fs::read_dir(state.path().join("boxes"))
        .unwrap()
        .map(|entry| {
            kiln_runtime::storage::read_json(&entry.unwrap().path().join("box.json")).unwrap()
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
    let failed: Value = kiln_runtime::storage::read_json(
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
#[ignore = "requires KVM and KILN_TEST_IMAGE"]
fn four_4g_clones_preserve_resources_and_enforce_total_quota() {
    let image = std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE");
    let state = state_directory();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let template = success(
        state.path(),
        &[
            "template",
            "build",
            "--image",
            &image,
            "--memory-mib",
            "4096",
            "--vcpus",
            "1",
            "--allow-unsafe-development",
        ],
    );
    assert_eq!(template["memory_mib"], 4096);
    assert_eq!(template["vcpus"], 1);
    let tid = template["id"].as_str().unwrap();
    assert_eq!(
        std::fs::metadata(state.path().join("snapshots").join(tid).join("memory.snap"))
            .unwrap()
            .len(),
        4 * 1024 * 1024 * 1024
    );
    let mut ids = Vec::new();
    for _ in 0..4 {
        let clone = success(state.path(), &["clone", tid, "--allow-unsafe-development"]);
        assert_eq!(clone["memory_mib"], 4096);
        assert_eq!(clone["vcpus"], 1);
        let id = clone["id"].as_str().unwrap().to_owned();
        let result = success(
            state.path(),
            &[
                "exec",
                &id,
                "--",
                "/bin/cat",
                "/proc/meminfo",
                "/proc/cpuinfo",
            ],
        );
        assert_eq!(result["exit_code"], 0);
        let output = result["stdout"].as_str().unwrap();
        let memory_kib: u64 = output
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!((3_900_000..4_194_304).contains(&memory_kib));
        assert_eq!(
            output
                .lines()
                .filter(|line| line.starts_with("processor\t"))
                .count(),
            1
        );
        ids.push(id);
    }
    let fifth = invoke(state.path(), &["clone", tid, "--allow-unsafe-development"]);
    assert!(!fifth.status.success());
    assert!(String::from_utf8_lossy(&fifth.stderr).contains("quota exceeded"));
    assert_eq!(
        success(state.path(), &["list"])["boxes"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    for id in ids {
        success(state.path(), &["delete", &id]);
    }
    let report = success(
        state.path(),
        &[
            "benchmark",
            "--image",
            &image,
            "--template",
            tid,
            "--samples",
            "1",
            "--allow-unsafe-development",
        ],
    );
    for mode in ["cold_boot", "template_restore"] {
        let sample = &report["results"][mode]["samples"][0];
        assert_eq!(sample["memory_mib"], 4096, "{mode}");
        assert_eq!(sample["vcpus"], 1, "{mode}");
    }
    success(state.path(), &["checkpoint", "delete", tid]);
}

#[test]
#[ignore = "requires KVM and KILN_TEST_IMAGE"]
fn simultaneous_starts_cannot_create_two_disk_writers() {
    let image = std::env::var("KILN_TEST_IMAGE").expect("set KILN_TEST_IMAGE");
    let state = state_directory();
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
    let run = run_directory(state.path(), &record);
    let executable = if record["jail"].is_object() {
        run.join("firecracker")
    } else {
        kiln_runtime::host::executable("firecracker").unwrap()
    };
    assert!(
        kiln_runtime::process::find(&run, &[&executable])
            .unwrap()
            .is_some()
    );
    std::fs::rename(run.join("api.sock"), run.join("api.unreachable")).unwrap();
    success(state.path(), &["stop", id, "--force"]);
    assert_eq!(success(state.path(), &["inspect", id])["state"], "stopped");
    success(state.path(), &["delete", id]);
}
