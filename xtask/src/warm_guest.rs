use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn cli(args: &[String]) -> i32 {
    if args.len() != 3 {
        eprintln!("usage: xtask warm-guest IMAGE_JSON FIRECRACKER OUTPUT_DIR");
        return 2;
    }
    match run(
        Path::new(&args[0]),
        Path::new(&args[1]),
        Path::new(&args[2]),
    ) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn run(image_path: &Path, firecracker: &Path, output: &Path) -> Result<(), String> {
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let image: Value =
        serde_json::from_str(&fs::read_to_string(image_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let mut cases = vec![
        "success",
        "fdstore",
        "credentials",
        "reset-failure",
        "marker-failure",
    ];
    if image["boot_mode"] == "systemd_warm_shared" {
        cases.push("identity-mismatch");
    }
    let mut results = Vec::new();
    for case in cases {
        let result = check_case(image_path, &image, firecracker, output, case)?;
        println!("{result}");
        results.push(result);
    }
    fs::write(
        output.join("results.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&results).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn check_case(
    image_path: &Path,
    image: &Value,
    firecracker: &Path,
    output: &Path,
    case: &str,
) -> Result<Value, String> {
    let boot_mode = image["boot_mode"].as_str().unwrap_or("");
    if boot_mode != "systemd_warm" && boot_mode != "systemd_warm_shared" {
        return Err(format!("unexpected boot mode {boot_mode}"));
    }
    let shared = boot_mode == "systemd_warm_shared";
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let disk = scratch.path().join("disk.ext4");
    let rootfs = image["rootfs_path"]
        .as_str()
        .ok_or("image has no rootfs_path")?;
    let status = Command::new("cp")
        .args(["--reflink=auto", "--sparse=always"])
        .arg(image_path.parent().unwrap_or(Path::new(".")).join(rootfs))
        .arg(&disk)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("cp of the rootfs failed".into());
    }
    if case == "success" {
        let real = scratch.path().join("systemctl");
        debugfs(
            &disk,
            &format!("dump /usr/bin/systemctl {}", real.display()),
        )?;
        debugfs(
            &disk,
            &format!("write {} /usr/bin/systemctl.real", real.display()),
        )?;
        debugfs(
            &disk,
            "set_inode_field /usr/bin/systemctl.real mode 0100755",
        )?;
        debugfs(&disk, "rm /usr/bin/systemctl")?;
        inject(
            &disk,
            scratch.path(),
            "/usr/bin/systemctl",
            "#!/bin/sh\ncase \"$1\" in daemon-reexec|daemon-reload) echo \"$1\" >> /run/test-manager-refresh ;; esac\nexec /usr/bin/systemctl.real \"$@\"\n",
            true,
        )?;
    }
    if case == "fdstore" || case == "credentials" {
        debugfs(&disk, "mkdir /etc/systemd/system/systemd-sysctl.service.d")?;
        let settings = if case == "fdstore" {
            inject(
                &disk,
                scratch.path(),
                "/fdstore.py",
                "import array, os, socket\nfd = os.memfd_create('synthetic-test-secret')\nos.write(fd, b'test-only-not-a-real-secret')\naddress = os.environ['NOTIFY_SOCKET'].replace('@', '\\0', 1)\ns = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)\ns.connect(address)\ns.sendmsg([b'FDSTORE=1\\nFDNAME=synthetic'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])\n",
                false,
            )?;
            "ExecStart=\nExecStart=/usr/bin/python3 /fdstore.py\nNotifyAccess=all\nFileDescriptorStoreMax=1\nFileDescriptorStorePreserve=yes\n"
        } else {
            "SetCredential=kiln-probe:synthetic-test-only\n"
        };
        inject(
            &disk,
            scratch.path(),
            "/etc/systemd/system/systemd-sysctl.service.d/probe.conf",
            &format!("[Service]\n{settings}"),
            false,
        )?;
    }
    if case == "reset-failure" || case == "marker-failure" {
        let real = scratch.path().join("busctl");
        debugfs(&disk, &format!("dump /usr/bin/busctl {}", real.display()))?;
        debugfs(
            &disk,
            &format!("write {} /usr/bin/busctl.real", real.display()),
        )?;
        debugfs(&disk, "set_inode_field /usr/bin/busctl.real mode 0100755")?;
        debugfs(&disk, "rm /usr/bin/busctl")?;
        let hook = if case == "reset-failure" {
            "if [ \"$1\" = call ]; then echo injected-reset-failure >&2; exit 42; fi\n"
        } else {
            "if [ \"$1\" = call ] && [ \"$2\" = org.freedesktop.DBus ]; then mkdir /run/kiln-initialized; fi\n"
        };
        inject(
            &disk,
            scratch.path(),
            "/usr/bin/busctl",
            &format!("#!/bin/sh\n{hook}exec /usr/bin/busctl.real \"$@\"\n"),
            true,
        )?;
    }
    let vsock = scratch.path().join("vsock");
    let kernel = image["kernel_path"]
        .as_str()
        .ok_or("image has no kernel_path")?;
    let boot_args = format!(
        "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/kiln-init kiln.warm=1 initcall_blacklist=load_umh quiet{}",
        if shared { " kiln.retain_pid1=1" } else { "" }
    );
    let config = json!({
        "boot-source": {
            "kernel_image_path": image_path.parent().unwrap_or(Path::new(".")).join(kernel),
            "boot_args": boot_args,
        },
        "machine-config": {"vcpu_count": 1, "mem_size_mib": 256},
        "drives": [{
            "drive_id": "rootfs",
            "path_on_host": disk,
            "is_root_device": true,
            "is_read_only": false,
        }],
        "vsock": {"guest_cid": 3, "uds_path": &vsock},
    });
    let config_path = scratch.path().join("config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let log_path = output.join(format!("{case}.log"));
    let log = File::create(&log_path).map_err(|error| error.to_string())?;
    let mut process = Command::new(firecracker)
        .args(["--no-api", "--config-file"])
        .arg(&config_path)
        .stdout(Stdio::from(
            log.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| error.to_string())?;
    let outcome = (|| {
        if case == "fdstore" || case == "credentials" {
            let expected = if case == "fdstore" {
                "systemd-sysctl.service active/exited, 1 stored FDs"
            } else {
                "template service contains credentials"
            };
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let text = fs::read_to_string(&log_path).unwrap_or_default();
                if text.contains(expected) {
                    return Ok(json!({"case": case, "rejected": expected}));
                }
                if Instant::now() > deadline {
                    let start = text.len().saturating_sub(4000);
                    return Err(text[start..].to_string());
                }
                if request(&vsock, &json!({"type": "hello", "version": 1})).is_ok() {
                    return Err("unsafe template exposed listener".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        wait_hello(&vsock, false)?;
        thread::sleep(Duration::from_secs(3));
        wait_hello(&vsock, false)?;
        let identity = if shared && case != "identity-mismatch" {
            "11111111111111111111111111111111"
        } else {
            "37aabbee1234567890abcdef98765432"
        };
        let execute = json!({
            "type": "exec",
            "argv": ["/bin/cat", "/etc/machine-id"],
            "cwd": null,
            "env": {},
            "timeout_ms": 1000,
        });
        let early = request(&vsock, &execute)?;
        if early["code"] != "not_initialized" {
            return Err(format!("expected not_initialized, got {early}"));
        }
        let mut entropy = [0u8; 64];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut entropy))
            .map_err(|error| error.to_string())?;
        let initialize = json!({
            "type": "initialize",
            "hostname": "box-wire-test",
            "machine_id": identity,
            "entropy": entropy.to_vec(),
        });
        let vsock_for_reset = vsock.clone();
        let initialize_for_reset = initialize.clone();
        let reset = thread::spawn(move || request(&vsock_for_reset, &initialize_for_reset));
        let mut replies = Vec::new();
        for _ in 0..12 {
            match request(&vsock, &execute) {
                Ok(reply) => {
                    if reply["type"] == "exec" {
                        if reply["exit_code"] != 0 {
                            return Err(format!("exec exit {}", reply["exit_code"]));
                        }
                        let stdout = byte_list(&reply["stdout"])?;
                        if stdout != format!("{identity}\n").into_bytes() {
                            return Err("machine id differed".into());
                        }
                        if case != "success" {
                            return Err("workload escaped failed handoff".into());
                        }
                    } else if reply["code"] != "not_initialized" {
                        return Err(format!("unexpected reply {reply}"));
                    }
                    replies.push(reply["type"].as_str().unwrap_or("").to_string());
                }
                Err(_) => replies.push("handoff-disconnect".into()),
            }
        }
        let result = reset
            .join()
            .map_err(|_| "initialize thread panicked".to_string())??;
        if case == "success" {
            if result["type"] != "initialized" {
                return Err(format!("initialize failed: {result}"));
            }
            wait_hello(&vsock, true)?;
            let stdout = byte_list(&request(&vsock, &execute)?["stdout"])?;
            if stdout != format!("{identity}\n").into_bytes() {
                return Err("initialized machine id differed".into());
            }
            let login_exec = with_argv(
                &execute,
                vec![
                    "/bin/sh",
                    "-c",
                    "test ! -e /run/nologin && systemctl is-active systemd-user-sessions.service",
                ],
            );
            let login = request(&vsock, &login_exec)?;
            if login["exit_code"] != 0 {
                return Err(format!(
                    "initialized guest still blocks user logins: {login}"
                ));
            }
            if byte_list(&login["stdout"])? != b"active\n" {
                return Err(format!("user sessions were not active: {login}"));
            }
            let refresh = request(
                &vsock,
                &with_argv(&execute, vec!["/bin/cat", "/run/test-manager-refresh"]),
            )?;
            if refresh["exit_code"] != 0 {
                return Err(format!("manager refresh marker missing: {refresh}"));
            }
            let expected_refresh = if shared {
                "daemon-reload\n"
            } else {
                "daemon-reexec\n"
            };
            if byte_list(&refresh["stdout"])? != expected_refresh.as_bytes() {
                return Err(format!("manager refresh differed: {refresh}"));
            }
        } else {
            if result["code"] != "initialization_failed" {
                return Err(format!("expected initialization_failed: {result}"));
            }
            wait_hello(&vsock, false)?;
            if request(&vsock, &execute)?["code"] != "not_initialized" {
                return Err("failed handoff became initialized".into());
            }
        }
        let log_text = fs::read_to_string(&log_path).unwrap_or_default();
        let timings: Vec<Value> = log_text
            .lines()
            .filter_map(|line| line.split_once("kiln_warm_reset ").map(|(_, rest)| rest))
            .map(|rest| serde_json::from_str(rest).map_err(|error| error.to_string()))
            .collect::<Result<_, _>>()?;
        if timings.len() != 1 {
            return Err(format!("expected one reset timing, got {timings:?}"));
        }
        let timing = &timings[0];
        if timing["retain_pid1"] != shared {
            return Err(format!("retain_pid1 differed: {timing}"));
        }
        let success = case == "success" || case == "marker-failure";
        if timing["success"] != success {
            return Err(format!("reset success differed: {timing}"));
        }
        let phases = timing["phases_ms"]
            .as_object()
            .ok_or("phases_ms is not an object")?;
        let phase_sum: u64 = phases.values().filter_map(Value::as_u64).sum();
        let total = timing["total_ms"]
            .as_u64()
            .ok_or("total_ms is not an integer")?;
        if phase_sum > total {
            return Err(format!("phases exceed total: {timing}"));
        }
        if timing["success"] == json!(true) {
            let keys: std::collections::BTreeSet<_> = phases.keys().cloned().collect();
            let expected = [
                "audit",
                "provision",
                "unmask",
                "manager_refresh",
                "start_services",
                "verify_identity",
            ]
            .into_iter()
            .map(str::to_string)
            .collect();
            if keys != expected {
                return Err(format!("phase names differed: {timing}"));
            }
        }
        if case == "identity-mismatch" && timing["phases_ms"] != json!({}) {
            return Err("identity rejection must precede guest mutation".into());
        }
        Ok(json!({
            "case": case,
            "initialize": result,
            "concurrent_replies": replies,
            "reset": timing,
        }))
    })();
    stop_process(&mut process);
    outcome
}

fn request(path: &Path, value: &Value) -> Result<Value, String> {
    let mut connection = UnixStream::connect(path).map_err(|error| error.to_string())?;
    connection
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    connection
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    connection
        .write_all(b"CONNECT 1024\n")
        .map_err(|error| error.to_string())?;
    let handshake = read_line(&mut connection)?;
    if !handshake.starts_with(b"OK ") {
        return Err(format!("vsock listener unavailable: {handshake:?}"));
    }
    let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    connection
        .write_all(&(payload.len() as u32).to_be_bytes())
        .map_err(|error| error.to_string())?;
    connection
        .write_all(&payload)
        .map_err(|error| error.to_string())?;
    let mut header = [0u8; 4];
    connection
        .read_exact(&mut header)
        .map_err(|_| "bootstrap listener handed off".to_string())?;
    let size = u32::from_be_bytes(header) as usize;
    if size > 1024 * 1024 {
        return Err("response exceeds 1MiB".into());
    }
    let mut body = vec![0u8; size];
    connection
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&body).map_err(|error| error.to_string())
}

fn read_line(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let count = stream.read(&mut byte).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    Ok(line)
}

fn wait_hello(path: &Path, initialized: bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let expected = json!({"type": "hello", "version": 1, "initialized": initialized});
    while Instant::now() < deadline {
        if request(path, &json!({"type": "hello", "version": 1}))
            .ok()
            .as_ref()
            == Some(&expected)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!("guest never reached initialized={initialized}"))
}

fn debugfs(disk: &Path, command: &str) -> Result<(), String> {
    let result = Command::new("debugfs")
        .args(["-w", "-R", command])
        .arg(disk)
        .output()
        .map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).to_string());
    }
    let stderr = String::from_utf8_lossy(&result.stderr);
    if ["File not found", "Could not", "not found by"]
        .iter()
        .any(|error| stderr.contains(error))
    {
        return Err(stderr.to_string());
    }
    Ok(())
}

fn inject(
    disk: &Path,
    scratch: &Path,
    guest_path: &str,
    contents: &str,
    executable: bool,
) -> Result<(), String> {
    let source = scratch.join("injected");
    fs::write(&source, contents).map_err(|error| error.to_string())?;
    debugfs(disk, &format!("write {} {guest_path}", source.display()))?;
    if executable {
        debugfs(disk, &format!("set_inode_field {guest_path} mode 0100755"))?;
    }
    Ok(())
}

fn byte_list(value: &Value) -> Result<Vec<u8>, String> {
    value
        .as_array()
        .ok_or("stdout is not a byte list")?
        .iter()
        .map(|item| {
            item.as_u64()
                .filter(|byte| *byte <= 255)
                .map(|byte| byte as u8)
                .ok_or_else(|| "stdout byte is out of range".to_string())
        })
        .collect()
}

fn with_argv(execute: &Value, argv: Vec<&str>) -> Value {
    let mut copy = execute.clone();
    copy["argv"] = json!(argv);
    copy
}

fn stop_process(process: &mut std::process::Child) {
    let _ = process.kill();
    let _ = process.wait();
}
