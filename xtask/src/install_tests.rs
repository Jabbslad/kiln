//! Unix tests for deploy/install.sh. Every stub is bash.
#![cfg(unix)]

use crate::register_cases;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const FRESH_CATALOG: &str = r#"[{"name":"ubuntu-4g","memory_mib":4096,"vcpus":1}]"#;
const UPGRADE_CATALOG: &str = r#"[{"name":"custom","memory_mib":3072,"vcpus":2}]"#;
const TEMPLATE_ID: &str = "dddddddddddddddddddddddddddddddd";
const X86_64: &str = "x86_64-unknown-linux-gnu";
const NAMES: &[&str] = &[
    "kiln",
    "kiln-runtime",
    "kiln-network",
    "kiln-host",
    "kiln-api",
];
const APP_UNITS: &[&str] = &[
    "kiln-app.service",
    "kiln-app-proxy.service",
    "kiln-app-proxy.socket",
];

const _: () = assert!(TEMPLATE_ID.len() == 32);

enum Member<'a> {
    File(&'a [u8]),
    Link(&'a str),
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn from_output(output: Output) -> Self {
        Self {
            code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn assert_ok(&self) {
        assert!(
            self.code == 0,
            "expected success\nstdout:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
    }

    fn assert_failed(&self) {
        assert!(
            self.code != 0,
            "expected failure\nstdout:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
    }
}

struct Fixture {
    root: PathBuf,
    package: PathBuf,
    stubs: PathBuf,
    state: PathBuf,
    log: PathBuf,
    curl_seq: PathBuf,
    curl_body: PathBuf,
    cp_marker: PathBuf,
    mv_marker: PathBuf,
    sleep_marker: PathBuf,
    script: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let package = root.join("package");
        let stubs = root.join("stubs");
        let state = root.join("stub-state");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(&stubs).unwrap();
        fs::create_dir_all(&state).unwrap();
        let script = package.join("install.sh");
        fs::copy(crate::repo_root().join("deploy/install.sh"), &script).unwrap();
        set_mode(&script, 0o755);
        let fx = Self {
            log: root.join("commands.log"),
            curl_seq: root.join("curl.seq"),
            curl_body: root.join("curl.body"),
            cp_marker: root.join("cp-failed"),
            mv_marker: root.join("mv-failed"),
            sleep_marker: root.join("sleep-cleared"),
            root,
            package,
            stubs,
            state,
            script,
            _tmp: tmp,
        };
        fx.write_stubs();
        fx
    }

    fn ready() -> Self {
        let fx = Self::new();
        fx.fresh_ready();
        fx
    }

    fn write_stubs(&self) {
        let log = text_path(&self.log);
        let state = text_path(&self.state);
        let passwd = text_path(&self.root.join("etc/passwd"));
        write_executable(&self.stubs.join("systemctl"), &systemctl_stub(&log, &state));
        write_executable(
            &self.stubs.join("systemd-analyze"),
            &log_exit_stub("systemd-analyze", &log),
        );
        write_executable(&self.stubs.join("useradd"), &useradd_stub(&log, &passwd));
        write_executable(
            &self.stubs.join("groupadd"),
            &log_exit_stub("groupadd", &log),
        );
        write_executable(
            &self.stubs.join("curl"),
            &curl_stub(
                &log,
                &text_path(&self.curl_seq),
                &text_path(&self.curl_body),
            ),
        );
        write_executable(
            &self.stubs.join("sleep"),
            &sleep_stub(&log, &text_path(&self.sleep_marker)),
        );
        write_executable(
            &self.stubs.join("cp"),
            &cp_stub(&log, &text_path(&self.cp_marker)),
        );
        write_executable(
            &self.stubs.join("mv"),
            &mv_stub(&log, &text_path(&self.mv_marker)),
        );
        for name in ["ip", "nft", "sysctl"] {
            write_executable(&self.stubs.join(name), &log_exit_stub(name, &log));
        }
        fs::write(self.state.join("killmode"), "process\n").unwrap();
    }

    fn invoke(&self, args: &[&str], extra: &[(&str, &str)]) -> Run {
        let mut command = Command::new("bash");
        command.arg(&self.script).args(args);
        command.env("KILN_INSTALL_FIXTURE", &self.root);
        let mut path = self.stubs.as_os_str().to_owned();
        path.push(":");
        if let Some(current) = env::var_os("PATH") {
            path.push(current);
        }
        command.env("PATH", path);
        for (key, value) in extra {
            command.env(key, value);
        }
        command.stdin(Stdio::null());
        Run::from_output(
            command
                .output()
                .unwrap_or_else(|error| panic!("bash {}: {error}", self.script.display())),
        )
    }

    fn lines(&self, prefix: Option<&str>) -> Vec<String> {
        if !self.log.exists() {
            return Vec::new();
        }
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter(|row| match prefix {
                Some(prefix) => row.starts_with(prefix),
                None => true,
            })
            .map(str::to_string)
            .collect()
    }

    fn any_line(&self, pred: impl Fn(&str) -> bool) -> bool {
        self.lines(None).iter().any(|line| pred(line))
    }

    fn clear_log(&self) {
        fs::write(&self.log, "").unwrap();
    }

    fn plan_curl(&self, steps: &[&str], body: &str) {
        let mut lines = Vec::new();
        for step in steps {
            match *step {
                "ok" => lines.push("ok".to_string()),
                "bad" => lines.push("bad".to_string()),
                "fail" => lines.extend(std::iter::repeat_n("fail".to_string(), 30)),
                other => panic!("unknown curl step {other}"),
            }
        }
        let text = if lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", lines.join("\n"))
        };
        fs::write(&self.curl_seq, text).unwrap();
        fs::write(&self.curl_body, body).unwrap();
    }

    fn host_skeleton(&self, distro: &str, version: &str) {
        for relative in [
            "etc",
            "opt",
            "usr/local",
            "var/lib",
            "run/systemd/system",
            "sys/fs/cgroup",
            "dev",
            "etc/systemd/system",
            "proc",
        ] {
            fs::create_dir_all(self.root.join(relative)).unwrap();
        }
        fs::write(
            self.root.join("etc/os-release"),
            format!("ID={distro}\nVERSION_ID={version}\n"),
        )
        .unwrap();
        fs::write(
            self.root.join("proc/meminfo"),
            "MemAvailable:       8388608 kB\n",
        )
        .unwrap();
        fs::write(
            self.root.join("sys/fs/cgroup/cgroup.controllers"),
            "cpu memory pids\n",
        )
        .unwrap();
        File::create(self.root.join("dev/kvm")).unwrap();
        fs::write(self.root.join("etc/passwd"), "").unwrap();
        fs::write(self.root.join("etc/group"), "").unwrap();
    }

    fn package_inputs(&self) {
        let kernel = b"kernel-bytes";
        let rootfs = b"rootfs-bytes";
        let image = self.package.join("image");
        fs::create_dir_all(&image).unwrap();
        fs::write(image.join("vmlinux"), kernel).unwrap();
        fs::write(image.join("rootfs.ext4"), rootfs).unwrap();
        fs::write(image.join("inputs.json"), "{}\n").unwrap();
        let manifest = serde_json::json!({
            "boot_mode": "systemd_warm",
            "architecture": "x86_64",
            "kernel_path": "vmlinux",
            "rootfs_path": "rootfs.ext4",
            "kernel_sha256": sha256_hex(kernel),
            "rootfs_sha256": sha256_hex(rootfs),
        });
        fs::write(image.join("image.json"), format!("{manifest}\n")).unwrap();
        let deploy = self.package.join("deploy");
        fs::create_dir_all(&deploy).unwrap();
        fs::copy(
            crate::repo_root().join("deploy/kiln-host.service"),
            deploy.join("kiln-host.service"),
        )
        .unwrap();
        fs::copy(
            crate::repo_root().join("deploy/kiln-api.service"),
            deploy.join("kiln-api.service"),
        )
        .unwrap();
        let bindir = self.package.join("bin");
        fs::create_dir_all(&bindir).unwrap();
        let log = text_path(&self.log);
        write_executable(
            &bindir.join("kiln-runtime"),
            &runtime_stub(&log, TEMPLATE_ID),
        );
        for name in ["kiln", "kiln-host", "kiln-api", "kiln-network"] {
            write_executable(&bindir.join(name), &bin_stub(name, &log));
        }
        write_executable(
            &self.package.join("fetch-firecracker.sh"),
            r#"#!/bin/bash
printf 'firecracker\n' > "$1/firecracker"
printf 'jailer\n' > "$1/jailer"
chmod 755 "$1/firecracker" "$1/jailer"
"#,
        );
    }

    fn fresh_ready(&self) {
        self.host_skeleton("ubuntu", "24.04");
        self.package_inputs();
        let libexec = self.root.join("usr/local/libexec");
        fs::create_dir(&libexec).unwrap();
        set_mode(&libexec, 0o750);
        self.plan_curl(&["ok"], FRESH_CATALOG);
    }

    fn stage_app_units(&self) {
        let deploy = self.package.join("deploy");
        fs::create_dir_all(&deploy).unwrap();
        for name in APP_UNITS {
            fs::copy(
                crate::repo_root().join("deploy").join(name),
                deploy.join(name),
            )
            .unwrap();
        }
    }

    fn write_dashboard_archive(
        &self,
        members: &[(&str, Member<'_>)],
        path: Option<&Path>,
    ) -> PathBuf {
        let path = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.join("dashboard.tar.gz"));
        let file = File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, member) in members {
            match member {
                Member::File(data) => append_bytes(&mut archive, name, data),
                Member::Link(target) => append_link(&mut archive, name, target),
            }
        }
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap();
        path
    }

    fn dashboard_dir(&self) -> PathBuf {
        self.root.join("usr/local/share/kiln/dashboard")
    }

    fn assert_no_dashboard_fetch(&self, result: &Run) {
        for line in self.lines(Some("curl ")) {
            assert!(!line.contains("dashboard"), "{line}");
            assert!(!line.contains("kiln-dashboard"), "{line}");
        }
        let blob = format!("{}{}", result.stdout, result.stderr);
        assert!(!blob.contains("wget"), "{blob}");
        let installer = fs::read_to_string(crate::repo_root().join("deploy/install.sh")).unwrap();
        let pattern = regex::Regex::new(r"https?://\S*dashboard").unwrap();
        assert!(!pattern.is_match(&installer));
    }
}

struct Upgrade {
    fx: Fixture,
    etc: PathBuf,
    opt: PathBuf,
    var_dir: PathBuf,
    destinations: BTreeMap<String, PathBuf>,
    identity: PathBuf,
    identity_rollback: PathBuf,
    journal: PathBuf,
    side: Vec<(PathBuf, u64, Vec<u8>)>,
    original: BTreeMap<String, Vec<u8>>,
    data: BTreeMap<PathBuf, Vec<u8>>,
}

impl Upgrade {
    fn new() -> Self {
        let fx = Fixture::new();
        fx.host_skeleton("ubuntu", "24.04");
        let etc = fx.root.join("etc/kiln");
        let opt = fx.root.join("opt/kiln");
        let var_dir = fx.root.join("var/lib/kiln");
        let libexec = fx.root.join("usr/local/libexec");
        for path in [&etc, &opt.join("bin"), &var_dir, &libexec] {
            fs::create_dir_all(path).unwrap();
        }
        let destinations = NAMES
            .iter()
            .map(|name| {
                let path = if matches!(*name, "kiln" | "kiln-runtime" | "kiln-network") {
                    opt.join("bin").join(name)
                } else {
                    libexec.join(name)
                };
                ((*name).to_string(), path)
            })
            .collect::<BTreeMap<_, _>>();
        let identity = libexec.join("kiln-identity");
        let identity_rollback = libexec.join("kiln-identity.rollback-v033");
        fs::write(&identity, b"identity-bytes-v1").unwrap();
        fs::write(&identity_rollback, b"handmade-rollback-v033").unwrap();
        let firecracker = opt.join("bin/firecracker");
        let jailer = opt.join("bin/jailer");
        fs::write(&firecracker, b"firecracker-1.17.0").unwrap();
        fs::write(&jailer, b"jailer-1.17.0").unwrap();
        let snap = var_dir.join("runtime/snapshots/a1323edbf1db1ff8bef3d5ccd9c6ebb6");
        fs::create_dir_all(&snap).unwrap();
        let snapshot = snap.join("memory.snap");
        fs::write(&snapshot, b"snapshot-marker-not-6gb").unwrap();
        fs::write(snap.join("disk.ext4"), b"disk-marker").unwrap();
        fs::create_dir_all(var_dir.join("runtime")).unwrap();
        fs::create_dir(var_dir.join("journal")).unwrap();
        fs::create_dir(var_dir.join("image")).unwrap();
        for directory in ["runtime", "journal", "image"] {
            fs::write(var_dir.join(directory).join("preserve"), directory).unwrap();
        }
        let journal = var_dir.join("journal/operations.sqlite");
        sqlite(
            &journal,
            "PRAGMA journal_mode=DELETE; PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);",
        );
        let host_json = serde_json::json!({
            "runtime_dir": var_dir.join("runtime").to_str().unwrap(),
            "journal_dir": var_dir.join("journal").to_str().unwrap(),
            "socket": "/run/kiln/host.sock",
            "isolation_config": etc.join("isolation.json").to_str().unwrap(),
            "templates": {"custom": "a".repeat(32)},
        });
        fs::write(
            etc.join("host.json"),
            serde_json::to_string(&host_json).unwrap(),
        )
        .unwrap();
        fs::write(etc.join("isolation.json"), "{}\n").unwrap();
        for name in ["ca.crt", "tls.crt", "tls.key", "admin.token"] {
            fs::write(etc.join(name), "synthetic-test-only").unwrap();
        }
        for (name, mode) in [("identity.json", 0o640), ("directory.token", 0o600)] {
            let path = etc.join(name);
            fs::write(&path, "synthetic-enrollment-only").unwrap();
            set_mode(&path, mode);
        }
        let side = [
            &identity,
            &identity_rollback,
            &snapshot,
            &firecracker,
            &jailer,
        ]
        .into_iter()
        .map(|path| {
            let meta = fs::metadata(path).unwrap();
            (path.clone(), meta.ino(), fs::read(path).unwrap())
        })
        .collect();
        let mut upgrade = Self {
            fx,
            etc,
            opt,
            var_dir,
            destinations,
            identity,
            identity_rollback,
            journal,
            side,
            original: BTreeMap::new(),
            data: BTreeMap::new(),
        };
        upgrade.write_release("0.4.4", X86_64, Some("0.3.0"));
        upgrade.write_versions("0.3.2", "0.4.4", "", "");
        upgrade.write_units("100.64.5.6", "enp7s0");
        upgrade.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
        upgrade.refresh_data();
        upgrade
    }

    fn write_versions(
        &mut self,
        installed: &str,
        target: &str,
        installed_extra: &str,
        target_extra: &str,
    ) {
        let bindir = self.fx.package.join("bin");
        fs::create_dir_all(&bindir).unwrap();
        for name in NAMES {
            let reported = reported(name);
            let (installed_text, target_text) = if *name == "kiln-network" {
                (
                    "#!/bin/sh\nexit 0\n".to_string(),
                    format!("#!/bin/sh\nexit 0\n{target_extra}"),
                )
            } else {
                (
                    format!("#!/bin/sh\necho '{reported} {installed}'\n{installed_extra}"),
                    format!("#!/bin/sh\necho '{reported} {target}'\n{target_extra}"),
                )
            };
            write_executable(&self.destinations[*name], &installed_text);
            write_executable(&bindir.join(name), &target_text);
        }
        self.original = self
            .destinations
            .iter()
            .map(|(name, path)| (name.clone(), fs::read(path).unwrap()))
            .collect();
    }

    fn write_release(&self, version: &str, target: &str, min_upgrade_from: Option<&str>) {
        let version = if version.starts_with('v') {
            version.to_string()
        } else {
            format!("v{version}")
        };
        let mut payload = serde_json::json!({
            "version": version,
            "target": target,
        });
        if let Some(min_from) = min_upgrade_from {
            payload["min_upgrade_from"] = serde_json::Value::String(min_from.to_string());
        }
        fs::write(
            self.fx.package.join("release.json"),
            serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();
    }

    fn write_units(&self, address: &str, uplink: &str) {
        let host = text_path(&self.destinations["kiln-host"]);
        let api = text_path(&self.destinations["kiln-api"]);
        let network = text_path(&self.destinations["kiln-network"]);
        let etc = text_path(&self.etc);
        fs::write(
            self.fx.state.join("exec-kiln-host.service"),
            format!(
                "{{ path={host} ; argv[]={host} --config {etc}/host.json ; ignore_errors=no ; }}\n"
            ),
        )
        .unwrap();
        fs::write(
            self.fx.state.join("exec-kiln-api.service"),
            format!(
                "{{ path={api} ; argv[]={api} --listen {address}:8443 --host-socket /run/kiln/host.sock --token-file {etc}/admin.token --tls-cert {etc}/tls.crt --tls-key {etc}/tls.key ; ignore_errors=no ; }}\n"
            ),
        )
        .unwrap();
        fs::write(
            self.fx.state.join("exec-kiln-network.service"),
            format!(
                "{{ path={network} ; argv[]={network} provision {uplink} ; ignore_errors=no ; }}\n"
            ),
        )
        .unwrap();
        for service in ["kiln-host.service", "kiln-api.service"] {
            fs::write(self.fx.state.join(format!("active-{service}")), "active\n").unwrap();
        }
    }

    fn refresh_data(&mut self) {
        self.data.clear();
        for base in [&self.etc, &self.var_dir] {
            collect_files(&self.fx.root, base, &mut self.data);
        }
    }

    fn assert_data_preserved(&self) {
        for (relative, data) in &self.data {
            let got = fs::read(self.fx.root.join(relative))
                .unwrap_or_else(|_| panic!("missing {}", relative.display()));
            assert_eq!(&got, data, "{}", relative.display());
        }
        assert_mode(&self.etc.join("identity.json"), 0o640);
        assert_mode(&self.etc.join("directory.token"), 0o600);
    }

    fn assert_original_binaries(&self) {
        for (name, path) in &self.destinations {
            assert_eq!(&fs::read(path).unwrap(), &self.original[name], "{name}");
        }
    }

    fn assert_untouched_neighbors(&self) {
        for (path, inode, data) in &self.side {
            let meta = fs::metadata(path).unwrap();
            assert_eq!(meta.ino(), *inode, "{}", path.display());
            assert_eq!(&fs::read(path).unwrap(), data, "{}", path.display());
        }
        assert!(!self.opt.join("libexec").exists());
        assert!(!self.opt.join("release.json").exists());
        assert!(!self.etc.join("release.json").exists());
        let upgrades = self.opt.join("upgrades");
        if upgrades.exists() {
            let mut stack = vec![upgrades];
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() && !is_symlink(&path) {
                        stack.push(path);
                    } else if path.is_file() {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        assert!(NAMES.contains(&name.as_str()), "{}", path.display());
                        assert!(!name.contains("snap"), "{}", path.display());
                    }
                }
            }
        }
    }
}

fn test_installer_names_binaries_and_keeps_the_production_lock_path() {
    let text = fs::read_to_string(crate::repo_root().join("deploy/install.sh")).unwrap();
    assert!(!text.contains("kiln-*"));
    assert!(text.contains("/run/lock/kiln-install.lock"));
    assert!(text.contains("/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin"));
}

fn test_private_addresses_fail_closed_and_local_bind_is_separate() {
    let fx = Fixture::new();
    fx.host_skeleton("ubuntu", "24.04");
    for value in [
        "0.0.0.0",
        "8.8.8.8",
        "172.15.255.254",
        "172.32.0.1",
        "100.128.0.1",
        "169.254.1.2",
        "192.0.2.3",
        "224.0.0.1",
        "::1",
        "host;id",
    ] {
        let result = fx.invoke(&["--address", value], &[]);
        assert!(result.code != 0, "{value}\n{}", result.stderr);
        assert_has(&result.stderr, "private");
        assert!(!fx.root.join("etc/kiln").exists(), "{value}");
    }
    for value in [
        "10.2.3.4",
        "172.16.0.1",
        "172.31.255.254",
        "192.168.3.7",
        "100.64.0.1",
        "100.127.255.254",
    ] {
        let result = fx.invoke(&["--address", value], &[]);
        assert!(result.code != 0, "{value}\n{}", result.stderr);
        assert_has(&result.stderr, "assigned");
        assert_lacks(&result.stderr, "private/VPN");
    }
}

fn test_supported_hosts_reach_systemd_and_other_hosts_stop_first() {
    let fx = Fixture::new();
    for (distro, version) in [("ubuntu", "24.04"), ("ubuntu", "26.04"), ("debian", "13")] {
        fx.host_skeleton(distro, version);
        fs::remove_dir_all(fx.root.join("run/systemd/system")).unwrap();
        let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
        assert_has(&result.stderr, "booted systemd");
        assert_lacks(&result.stderr, "supports Ubuntu");
    }
    write_executable(
        &fx.stubs.join("uname"),
        "#!/bin/sh\nprintf '%s\\n' \"uname $*\" >> \"$KILN_LOG\"\nif [ \"$1\" = -m ]; then printf '%s\\n' aarch64; exit 0; fi\nexit 0\n",
    );
    let log = text_path(&fx.log);
    for (distro, version) in [
        ("ubuntu", "22.04"),
        ("ubuntu", "24.10"),
        ("debian", "12"),
        ("ubuntu", "26.04"),
    ] {
        fx.clear_log();
        fx.host_skeleton(distro, version);
        fs::remove_dir_all(fx.root.join("run/systemd/system")).unwrap();
        let result = fx.invoke(&["--address", "127.0.0.1"], &[("KILN_LOG", &log)]);
        assert_has(&result.stderr, "supports Ubuntu");
        assert_lacks(&result.stderr, "booted systemd");
        if (distro, version) == ("ubuntu", "26.04") {
            assert!(!fx.lines(Some("uname")).is_empty(), "{distro} {version}");
        } else {
            assert!(fx.lines(Some("uname")).is_empty(), "{distro} {version}");
        }
    }
}

fn test_low_memory_stops_before_the_port_bind() {
    let fx = Fixture::new();
    fx.host_skeleton("ubuntu", "24.04");
    fs::write(
        fx.root.join("proc/meminfo"),
        "MemAvailable:       1024 kB\n",
    )
    .unwrap();
    let result = fx.invoke(&["--address", "100.64.5.6"], &[]);
    assert_has(&result.stderr, "6 GiB");
    assert_lacks(&result.stderr, "assigned");
}

fn test_identity_range_and_destination_conflicts_stop_before_install() {
    let fx = Fixture::new();
    fx.host_skeleton("ubuntu", "24.04");
    fs::write(fx.root.join("etc/subuid"), "alice:69999:2\n").unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&result.stderr, "reserved range");
    fs::write(fx.root.join("etc/subuid"), "alice:70008:9\n").unwrap();
    fs::write(fx.root.join("etc/passwd"), "other:x:70000:1:::\n").unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&result.stderr, "UID range");
    fs::write(fx.root.join("etc/passwd"), "").unwrap();
    let status = fx.root.join("proc/4242/status");
    fs::create_dir(status.parent().unwrap()).unwrap();
    fs::write(&status, "Uid:\t70000\t0\t0\t0\n").unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&result.stderr, "jail identity");
    fs::remove_dir_all(fx.root.join("proc/4242")).unwrap();
    let libexec = fx.root.join("usr/local/libexec");
    fs::create_dir(&libexec).unwrap();
    let host = libexec.join("kiln-host");
    fs::write(&host, "keep").unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(
        &result.stderr,
        &format!("refusing existing destination {}", host.display()),
    );
    fs::remove_file(&host).unwrap();
    let api = libexec.join("kiln-api");
    symlink(fx.root.join("missing"), &api).unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(
        &result.stderr,
        &format!("refusing existing destination {}", api.display()),
    );
    fs::remove_file(&api).unwrap();
    let etc = fx.root.join("etc/kiln");
    fs::create_dir(&etc).unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&result.stderr, "incomplete installation");
    assert!(fx.lines(Some("systemctl stop")).is_empty());
    fs::remove_dir(&etc).unwrap();
    fs::write(fx.state.join("load-state"), "loaded\n").unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&result.stderr, "existing kiln-cgroup.service");
}

fn test_network_preflight_rejects_injection_and_missing_nsenter_before_ip() {
    let fx = Fixture::ready();
    let result = fx.invoke(
        &["--address", "127.0.0.1", "--network-uplink", "eth0;id"],
        &[],
    );
    assert_has(&result.stderr, "interface");
    assert!(fx.lines(Some("ip ")).is_empty());
    fx.clear_log();
    let result = fx.invoke(
        &["--address", "127.0.0.1", "--network-uplink", "eth0"],
        &[("KILN_INSTALL_MISSING", "nsenter")],
    );
    assert_has(&result.stderr, "util-linux");
    assert!(fx.lines(Some("ip ")).is_empty());
    let net = fx.root.join("sys/class/net/kiln0");
    fs::create_dir_all(net.parent().unwrap()).unwrap();
    fs::create_dir(&net).unwrap();
    let result = fx.invoke(&["--address", "127.0.0.1", "--network-uplink", "eth0"], &[]);
    assert_has(&result.stderr, "refusing existing destination");
    assert_has(&result.stderr, "kiln0");
}

fn test_check_only_does_not_install() {
    let fx = Fixture::ready();
    let result = fx.invoke(&["--address", "127.0.0.1"], &[]);
    result.assert_ok();
    assert_has(&result.stdout, "Check only");
    assert_has(&result.stdout, "/usr/local/libexec/kiln-host");
    assert_lacks(&result.stdout, "kiln-*");
    assert!(!fx.root.join("etc/kiln").exists());
    assert!(!fx.root.join("opt/kiln").exists());
    assert_mode(&fx.root.join("usr/local/libexec"), 0o750);
}

fn test_dashboard_archive_extracts_and_enables_the_loopback_unit() {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let archive = fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>dashboard</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
            ("release.json", Member::File(b"{\"version\":\"v0.1.0\"}\n")),
        ],
        None,
    );
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    result.assert_ok();
    let dest = fx.dashboard_dir();
    assert_eq!(
        fs::read(dest.join("index.html")).unwrap(),
        b"<html>dashboard</html>\n"
    );
    assert_hides_other_writes(&dest);
    assert_hides_other_writes(&dest.join("index.html"));
    assert_extract_matches(&archive, &dest);
    let units = fx.root.join("etc/systemd/system");
    for name in APP_UNITS {
        assert_eq!(
            fs::read(units.join(name)).unwrap(),
            fs::read(crate::repo_root().join("deploy").join(name)).unwrap(),
            "{name}"
        );
    }
    let lines = fx.lines(None);
    assert!(
        lines
            .iter()
            .any(|line| line == "systemctl enable kiln-app.service")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "systemctl enable --now kiln-app-proxy.socket")
    );
    assert!(units.join("kiln-api.service").is_file());
    fx.assert_no_dashboard_fetch(&result);
}

fn test_socket_enable_failure_shows_systemctl_stderr() {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let archive = fx.write_dashboard_archive(
        &[("index.html", Member::File(b"<html>dashboard</html>\n"))],
        None,
    );
    fs::write(fx.state.join("fail-enable-socket"), "1\n").unwrap();
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    result.assert_failed();
    assert_has(&result.stderr, "socket unit is masked");
    assert_has(
        &result.stderr,
        "systemctl enable --now kiln-app-proxy.socket",
    );
}

fn test_apply_without_dashboard_archive_leaves_the_share_path_absent() {
    let fx = Fixture::ready();
    let result = fx.invoke(&["--address", "127.0.0.1", "--apply"], &[]);
    result.assert_ok();
    assert!(!fx.dashboard_dir().exists());
    assert!(!fx.root.join("usr/local/share/kiln").exists());
    let units = fx.root.join("etc/systemd/system");
    assert!(units.join("kiln-api.service").is_file());
    for name in APP_UNITS {
        assert!(!units.join(name).exists(), "{name}");
    }
    assert!(!fx.any_line(|line| line.contains("kiln-app")));
    fx.assert_no_dashboard_fetch(&result);
}

fn test_dashboard_archive_without_apply_writes_nothing() {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let archive = fx.write_dashboard_archive(
        &[("index.html", Member::File(b"<html>dashboard</html>\n"))],
        None,
    );
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    result.assert_ok();
    assert_has(&result.stdout, "Check only");
    assert!(!fx.dashboard_dir().exists());
    assert!(!fx.root.join("etc/kiln").exists());
    let units = fx.root.join("etc/systemd/system");
    for name in APP_UNITS {
        assert!(!units.join(name).exists(), "{name}");
    }
    assert!(!fx.any_line(|line| line.contains("kiln-app")));
    fx.assert_no_dashboard_fetch(&result);
}

fn test_symlink_dashboard_member_exits_before_the_unit_is_enabled() {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let archive = fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>dashboard</html>\n")),
            ("assets/link.js", Member::Link("index.html")),
        ],
        None,
    );
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    result.assert_failed();
    assert_has(&result.stderr, "symlink");
    assert!(!fx.dashboard_dir().exists());
    assert!(!fx.any_line(|line| line.contains("kiln-app") && line.contains("enable")));
    let linked = fx.root.join("linked.tar.gz");
    symlink(&archive, &linked).unwrap();
    let linked_arg = text_path(&linked);
    let again = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &linked_arg,
        ],
        &[],
    );
    again.assert_failed();
    assert_has(&again.stderr, "regular file");
    assert!(!fx.root.join("etc/systemd/system/kiln-app.service").exists());
}

fn test_dashboard_dotdot_member_is_refused_before_writes() {
    assert_dashboard_member_refused("assets/..");
}

fn test_dashboard_dotdot_child_is_refused_before_writes() {
    assert_dashboard_member_refused("assets/../index.html");
}

fn assert_dashboard_member_refused(name: &str) {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let archive = fx.write_dashboard_archive(
        &[
            (name, Member::File(b"pwned\n")),
            ("index.html", Member::File(b"<html></html>\n")),
        ],
        None,
    );
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    assert!(!fx.root.join("etc/kiln").exists());
    result.assert_failed();
    assert_has(&result.stderr, &format!("not a safe relative path: {name}"));
    assert!(!fx.dashboard_dir().exists());
}

fn test_symlink_dashboard_destination_is_refused_before_writes() {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let share = fx.root.join("usr/local/share/kiln");
    fs::create_dir_all(&share).unwrap();
    let target = fx.root.join("kept-dashboard");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), "safe").unwrap();
    let dashboard = share.join("dashboard");
    symlink(&target, &dashboard).unwrap();
    let archive = fx.write_dashboard_archive(
        &[("index.html", Member::File(b"<html>dashboard</html>\n"))],
        None,
    );
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    result.assert_failed();
    assert_has(&result.stderr, "refusing symlink dashboard destination");
    assert!(is_symlink(&dashboard));
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "safe");
    assert!(!fx.root.join("etc/kiln").exists());
    assert!(!fx.root.join("opt/kiln").exists());
    assert!(!fx.root.join("etc/systemd/system/kiln-app.service").exists());
}

fn test_symlink_share_ancestor_is_refused_before_writes() {
    let fx = Fixture::ready();
    fx.stage_app_units();
    let parent = fx.root.join("usr/local/share");
    fs::create_dir_all(&parent).unwrap();
    let target = fx.root.join("kept-share");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), "safe").unwrap();
    let share = parent.join("kiln");
    symlink(&target, &share).unwrap();
    let archive = fx.write_dashboard_archive(
        &[("index.html", Member::File(b"<html>dashboard</html>\n"))],
        None,
    );
    let archive_arg = text_path(&archive);
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--dashboard-archive",
            &archive_arg,
        ],
        &[],
    );
    result.assert_failed();
    assert_has(
        &result.stderr,
        &format!("symlink destination ancestor: {}", share.display()),
    );
    assert!(is_symlink(&share));
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "safe");
    assert!(!fx.root.join("etc/kiln").exists());
    assert!(!fx.root.join("opt/kiln").exists());
}

fn test_packaged_door_unit_has_no_dashboard_root_and_installer_skips_caddy() {
    let door = fs::read_to_string(crate::repo_root().join("deploy/kiln-door.service")).unwrap();
    let installer = fs::read_to_string(crate::repo_root().join("deploy/install.sh")).unwrap();
    assert!(!door.contains("dashboard-root"));
    assert!(!installer.contains("/opt/paseo/Caddyfile"));
    assert!(!installer.contains("redirect_uri"));
}

fn test_apply_installs_certificates_units_and_network_without_prompt() {
    let fx = Fixture::ready();
    let result = fx.invoke(
        &[
            "--address",
            "127.0.0.1",
            "--apply",
            "--network-uplink",
            "eth0",
        ],
        &[],
    );
    result.assert_ok();
    assert_has(&result.stdout, "Host admin API: https://127.0.0.1:8443");
    assert_has(&result.stdout, "enroll --url https://127.0.0.1:8443");
    let etc = fx.root.join("etc/kiln");
    let crt = text_path(&etc.join("tls.crt"));
    let ca = text_path(&etc.join("ca.crt"));
    let cert = openssl(&["x509", "-in", &crt, "-noout", "-text"]);
    cert.assert_ok();
    assert_has(&cert.stdout, "IP Address:127.0.0.1");
    assert_has(&cert.stdout, "CA:FALSE");
    openssl(&["verify", "-CAfile", &ca, &crt]).assert_ok();
    let token = fs::read_to_string(etc.join("admin.token")).unwrap();
    let token = token.trim();
    assert!(
        regex::Regex::new(r"^[0-9a-f]{64}$")
            .unwrap()
            .is_match(token),
        "{token}"
    );
    for name in ["admin.token", "tls.key", "ca.key", "laptop.tar.gz"] {
        assert_mode(&etc.join(name), 0o600);
    }
    assert_lacks(&fs::read_to_string(etc.join("CONNECT.txt")).unwrap(), token);
    let laptop = etc.join("laptop.tar.gz");
    let names = tar_names(&laptop);
    let expected = BTreeSet::from([
        "kiln-connection/admin.token".to_string(),
        "kiln-connection/ca.crt".to_string(),
        "kiln-connection/CONNECT.txt".to_string(),
    ]);
    assert_eq!(names, expected);
    let connect = tar_bytes(&laptop, "kiln-connection/CONNECT.txt");
    assert!(
        connect
            .windows(b"https://127.0.0.1:8443".len())
            .any(|window| window == b"https://127.0.0.1:8443")
    );
    let curl_log = fx.lines(Some("curl ")).join("\n");
    assert_lacks(&curl_log, token);
    assert_lacks(&curl_log, "Bearer");
    assert_has(&curl_log, "-H @-");
    assert_lacks(&fs::read_to_string(&fx.log).unwrap(), token);
    let opt = fx.root.join("opt/kiln");
    let units = fx.root.join("etc/systemd/system");
    assert_eq!(
        fs::read(units.join("kiln-host.service")).unwrap(),
        fs::read(crate::repo_root().join("deploy/kiln-host.service")).unwrap()
    );
    assert_eq!(
        fs::read(units.join("kiln-api.service")).unwrap(),
        fs::read(crate::repo_root().join("deploy/kiln-api.service")).unwrap()
    );
    let group = fs::read_to_string(units.join("kiln-cgroup.service")).unwrap();
    assert_has(&group, "+cpu +memory +pids");
    assert_has(&group, "RemainAfterExit=yes");
    let host_drop = fs::read_to_string(units.join("kiln-host.service.d/installer.conf")).unwrap();
    assert_has(&host_drop, "Requires=kiln-cgroup.service");
    assert_has(&host_drop, "After=kiln-cgroup.service");
    assert_mode(&units.join("kiln-host.service.d"), 0o700);
    let api = fs::read_to_string(units.join("kiln-api.service.d/installer.conf")).unwrap();
    assert_has(&api, "ExecStart=\n");
    assert_has(&api, "--listen 127.0.0.1:8443");
    assert_lacks(&api, "0.0.0.0");
    let network = fs::read_to_string(units.join("kiln-network.service")).unwrap();
    assert_has(
        &network,
        "ExecStart=/opt/kiln/bin/kiln-network provision eth0\n",
    );
    assert_has(
        &fs::read_to_string(units.join("kiln-host.service.d/network.conf")).unwrap(),
        "Requires=kiln-network.service",
    );
    let config = json_at(&etc.join("host.json"));
    assert_eq!(config["allow_unsafe_development"].as_bool(), Some(false));
    assert_eq!(
        config["templates"].clone(),
        serde_json::json!({"ubuntu-4g": TEMPLATE_ID})
    );
    assert_eq!(
        config["runtime_dir"].as_str(),
        Some("/var/lib/kiln/runtime")
    );
    let policy = json_at(&etc.join("isolation.json"));
    assert_eq!(policy["uid_base"].as_u64(), Some(70_000));
    assert_eq!(policy["gid_base"].as_u64(), Some(71_000));
    assert_eq!(policy["disk_backend"].as_str(), Some("copy"));
    assert_eq!(
        policy["network"].clone(),
        serde_json::json!({
            "helper": "/opt/kiln/bin/kiln-network",
            "namespace_scope": "kiln",
            "resolver": "1.1.1.1",
        })
    );
    assert_mode(&fx.root.join("usr/local/libexec"), 0o750);
    assert_mode(&etc, 0o755);
    assert_mode(&fx.root.join("var/lib/kiln"), 0o700);
    assert_mode(&opt, 0o755);
    assert!(opt.join("bin/firecracker").is_file());
    let runtime = fx.lines(Some("runtime PATH="));
    assert!(!runtime.is_empty());
    let value = runtime[0].split_once('=').unwrap().1;
    assert!(
        value.starts_with(&format!("{}:", opt.join("bin").display())),
        "{value}"
    );
    let rows = fx.lines(None);
    let template_at = rows
        .iter()
        .position(|line| line.contains("template") && line.contains("build"))
        .unwrap();
    let enable_at = rows
        .iter()
        .position(|line| line.starts_with("systemctl enable"))
        .unwrap();
    assert!(template_at < enable_at);
    assert_has(&rows[template_at], "--profile");
    assert_has(&rows[template_at], "isolated");
    assert!(!fx.lines(Some("ip link show dev eth0")).is_empty());
}

fn test_failed_readiness_disables_services_and_keeps_state() {
    let fx = Fixture::ready();
    fx.plan_curl(&["fail"], FRESH_CATALOG);
    let result = fx.invoke(&["--address", "127.0.0.1", "--apply"], &[]);
    result.assert_failed();
    assert_has(&result.stderr, "readiness");
    assert_eq!(
        fx.lines(Some("systemctl disable")),
        vec![
            "systemctl disable --now kiln-api.service".to_string(),
            "systemctl disable --now kiln-host.service".to_string(),
            "systemctl disable --now kiln-cgroup.service".to_string(),
        ]
    );
    assert!(fx.root.join("var/lib/kiln").is_dir());
    let config = json_at(&fx.root.join("etc/kiln/host.json"));
    assert_eq!(
        config["templates"].clone(),
        serde_json::json!({"ubuntu-4g": TEMPLATE_ID})
    );
    assert_mode(&fx.root.join("usr/local/libexec"), 0o750);
}

fn test_update_preserves_configuration_identity_and_snapshot() {
    let up = Upgrade::new();
    let result = up.fx.invoke(&["--apply"], &[]);
    result.assert_ok();
    assert_has(&result.stdout, "kiln login");
    for (name, path) in &up.destinations {
        assert_eq!(
            &fs::read(path).unwrap(),
            &fs::read(up.fx.package.join("bin").join(name)).unwrap(),
            "{name}"
        );
    }
    up.assert_data_preserved();
    up.assert_untouched_neighbors();
    let actions: Vec<_> = up
        .fx
        .lines(Some("systemctl "))
        .into_iter()
        .filter(|line| line.starts_with("systemctl stop") || line.starts_with("systemctl start"))
        .collect();
    assert_eq!(
        actions,
        vec![
            "systemctl stop kiln-api.service".to_string(),
            "systemctl stop kiln-host.service".to_string(),
            "systemctl start kiln-host.service".to_string(),
            "systemctl start kiln-api.service".to_string(),
        ]
    );
    assert!(
        !up.fx
            .lines(None)
            .iter()
            .any(|line| line == "systemctl stop kiln-network.service")
    );
    let backups = glob_named(&up.opt.join("upgrades"), "kiln-api");
    assert_eq!(backups.len(), 1);
    assert_eq!(child_names(backups[0].parent().unwrap()), sorted_names());
    assert_eq!(fs::read(&backups[0]).unwrap(), up.original["kiln-api"]);
    assert!(!up.opt.join("pending-upgrade.json").exists());
    up.fx.clear_log();
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let again = up.fx.invoke(&["--apply"], &[]);
    again.assert_ok();
    assert_has(&again.stdout, "already current");
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
    assert_eq!(glob_named(&up.opt.join("upgrades"), "kiln-api"), backups);
    up.assert_untouched_neighbors();
}

fn test_upgrade_disables_the_public_door_socket() {
    let up = Upgrade::new();
    let unit = up.root_unit("kiln-door-proxy.service");
    fs::write(
        &unit,
        "[Service]\nSockets=kiln-door-proxy.socket kiln-door-proxy-public.socket\n",
    )
    .unwrap();
    fs::write(up.fx.state.join("load-state"), "loaded\n").unwrap();
    let result = up.fx.invoke(&["--apply"], &[]);
    result.assert_ok();
    assert_eq!(
        fs::read_to_string(&unit).unwrap(),
        "[Service]\nSockets=kiln-door-proxy.socket\n"
    );
    assert!(
        up.fx
            .lines(None)
            .iter()
            .any(|line| line == "systemctl disable --now kiln-door-proxy-public.socket")
    );
    assert!(
        !up.fx
            .any_line(|line| line.contains("enable") && line.contains("kiln-door-proxy-public"))
    );
    assert_has(
        &result.stdout,
        "Binaries are already updated. Closing public tcp/8443.",
    );
    assert_has(&result.stdout, "Host admin API: https://100.64.5.6:8443");
    assert_lacks(&result.stdout, "Ready: https://");
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let commented = "# kiln-door-proxy-public.socket stays named only in this comment\n[Service]\nSockets=kiln-door-proxy.socket\n";
    fs::write(&unit, commented).unwrap();
    up.fx.clear_log();
    let again = up.fx.invoke(&["--apply"], &[]);
    again.assert_ok();
    assert_eq!(fs::read_to_string(&unit).unwrap(), commented);
    assert!(up.fx.lines(Some("systemctl daemon-reload")).is_empty());
    assert!(up.fx.lines(Some("systemctl restart")).is_empty());
}

fn test_dark_forge_layout_upgrades_and_rolls_back_without_release_json() {
    let mut up = Upgrade::new();
    up.write_release("0.4.5", X86_64, None);
    up.write_versions("0.4.4", "0.4.5", "", "");
    up.refresh_data();
    let result = up.fx.invoke(&["--apply"], &[]);
    result.assert_ok();
    assert_has(&result.stdout, "0.4.4 -> 0.4.5");
    assert!(!(up.opt.join("upgrades").exists() && up.opt.join("pending-upgrade.json").exists()));
    up.assert_data_preserved();
    up.assert_untouched_neighbors();
    let backups = children_starting(&up.opt.join("upgrades"), "0.4.4-");
    assert_eq!(backups.len(), 1);
    assert_eq!(child_names(&backups[0]), sorted_names());

    up.write_versions("0.4.4", "0.4.5", "", "");
    up.fx.plan_curl(&["ok", "fail", "ok"], UPGRADE_CATALOG);
    fs::remove_dir_all(up.opt.join("upgrades")).unwrap();
    let failed = up.fx.invoke(&["--apply"], &[]);
    failed.assert_failed();
    assert_has(&failed.stderr, "restored");
    up.assert_original_binaries();
    up.assert_untouched_neighbors();
    assert!(!up.opt.join("pending-upgrade.json").exists());
    assert!(any_named(&up.opt.join("upgrades"), "kiln-api"));
}

fn test_newer_patch_and_minor_releases_replace_binaries() {
    let mut up = Upgrade::new();
    for (installed, target, field) in [
        ("0.4.4", "0.4.5", false),
        ("0.4.4", "0.4.10", false),
        ("0.9.0", "0.10.0", true),
        ("0.3.99", "0.4.4", true),
    ] {
        let min_from = if field { Some("0.3.0") } else { None };
        up.write_release(target, X86_64, min_from);
        up.write_versions(installed, target, "", "");
        up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
        let result = up.fx.invoke(&["--apply"], &[]);
        result.assert_ok();
        assert_eq!(
            fs::read(&up.destinations["kiln"]).unwrap(),
            fs::read(up.fx.package.join("bin/kiln")).unwrap()
        );
        up.assert_untouched_neighbors();
        up.write_versions(target, target, "", "");
        up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
        let again = up.fx.invoke(&["--apply"], &[]);
        assert_has(&again.stdout, "already current");
    }
}

fn test_downgrade_floor_and_missing_guard_stop_before_any_change() {
    let mut up = Upgrade::new();
    let cases = [
        (
            "0.4.5",
            "0.4.4",
            None,
            "refusing downgrade from 0.4.5 to 0.4.4",
        ),
        (
            "0.4.10",
            "0.4.4",
            None,
            "refusing downgrade from 0.4.10 to 0.4.4",
        ),
        (
            "0.10.0",
            "0.9.0",
            None,
            "refusing downgrade from 0.10.0 to 0.9.0",
        ),
        (
            "0.4.4",
            "0.4.6",
            Some("0.4.5"),
            "package requires min_upgrade_from 0.4.5",
        ),
        ("0.2.2", "0.4.4", None, "older than 0.3.0"),
        ("0.3.0", "0.4.4", None, "without min_upgrade_from"),
        ("0.9.0", "0.10.0", None, "without min_upgrade_from"),
        ("0.4.0", "0.4.5-rc.1", None, "unsupported update package"),
        ("0.4.4", "v0.4", None, "unsupported update package"),
    ];
    for (installed, target, extra, message) in cases {
        up.fx.clear_log();
        up.write_release(target, X86_64, extra);
        up.write_versions(installed, target, "", "");
        let result = up.fx.invoke(&["--apply"], &[]);
        result.assert_failed();
        assert_has(&result.stderr, message);
        assert!(up.fx.lines(Some("systemctl stop")).is_empty());
        assert!(up.fx.lines(Some("curl ")).is_empty());
        assert!(!up.opt.join("upgrades").exists());
        assert!(!up.opt.join("pending-upgrade.json").exists());
        up.assert_original_binaries();
        up.assert_untouched_neighbors();
    }
    up.write_release("0.4.4", "aarch64", None);
    up.write_versions("0.4.4", "0.4.4", "", "");
    let result = up.fx.invoke(&["--apply"], &[]);
    assert_has(&result.stderr, "unsupported update package");
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
}

fn test_same_version_replaces_only_different_bytes() {
    let mut up = Upgrade::new();
    up.write_release("0.4.4", X86_64, Some("0.3.0"));
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let same = up.fx.invoke(&["--apply"], &[]);
    same.assert_ok();
    assert_has(
        &same.stdout,
        "Kiln 0.4.4 is already current; nothing changed.",
    );
    assert!(!up.opt.join("upgrades").exists());
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
    up.write_versions("0.4.4", "0.4.4", "", "# rebuilt\n");
    up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
    let changed = up.fx.invoke(&["--apply"], &[]);
    changed.assert_ok();
    assert!(
        fs::read_to_string(&up.destinations["kiln"])
            .unwrap()
            .contains("# rebuilt")
    );
    up.assert_untouched_neighbors();
}

fn test_dashboard_extract_replaces_only_different_bytes() {
    let mut up = Upgrade::new();
    up.write_release("0.4.4", X86_64, Some("0.3.0"));
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.stage_app_units();
    let archive = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>one</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let archive_arg = text_path(&archive);
    let first = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &archive_arg], &[]);
    first.assert_ok();
    let index = up.fx.dashboard_dir().join("index.html");
    let stamp = file_stamp(&index);
    assert_eq!(fs::read(&index).unwrap(), b"<html>one</html>\n");
    assert_extract_matches(&archive, &up.fx.dashboard_dir());
    up.assert_data_preserved();
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let second = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &archive_arg], &[]);
    second.assert_ok();
    assert_eq!(file_stamp(&index), stamp);
    assert_eq!(fs::read(&index).unwrap(), b"<html>one</html>\n");
    up.assert_data_preserved();
    let changed = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>two</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let changed_arg = text_path(&changed);
    let third = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &changed_arg], &[]);
    third.assert_ok();
    assert_eq!(fs::read(&index).unwrap(), b"<html>two</html>\n");
    assert_ne!(file_stamp(&index), stamp);
    assert_extract_matches(&changed, &up.fx.dashboard_dir());
    up.assert_data_preserved();
    up.fx.assert_no_dashboard_fetch(&third);
}

fn test_failed_dashboard_replace_keeps_the_installed_tree() {
    let mut up = Upgrade::new();
    up.write_release("0.4.4", X86_64, Some("0.3.0"));
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.stage_app_units();
    let archive = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>one</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let archive_arg = text_path(&archive);
    let first = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &archive_arg], &[]);
    first.assert_ok();
    let index = up.fx.dashboard_dir().join("index.html");
    assert_eq!(fs::read(&index).unwrap(), b"<html>one</html>\n");
    let changed = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>two</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let changed_arg = text_path(&changed);
    let failed = up.fx.invoke(
        &["--apply", "--dashboard-archive", &changed_arg],
        &[("KILN_MV_FAIL_DASHBOARD", "1")],
    );
    failed.assert_failed();
    assert_has(&failed.stderr, "cannot install dashboard archive");
    assert_eq!(fs::read(&index).unwrap(), b"<html>one</html>\n");
    assert_eq!(
        dashboard_asides(up.fx.dashboard_dir().parent().unwrap()),
        Vec::<PathBuf>::new()
    );
    up.assert_data_preserved();
}

fn test_rerun_after_killed_dashboard_swap_clears_asides() {
    let mut up = Upgrade::new();
    up.write_release("0.4.4", X86_64, Some("0.3.0"));
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.stage_app_units();
    let archive = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>one</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let archive_arg = text_path(&archive);
    let first = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &archive_arg], &[]);
    first.assert_ok();
    let share = up.fx.dashboard_dir().parent().unwrap().to_path_buf();
    fs::write(share.join("keep"), "keep\n").unwrap();
    let old = share.join(".dashboard.oldtree");
    let killed = share.join(".dashboard.killed");
    fs::rename(up.fx.dashboard_dir(), &old).unwrap();
    fs::create_dir(&killed).unwrap();
    fs::write(killed.join("index.html"), b"<html>killed</html>\n").unwrap();
    let changed = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>two</html>\n")),
            ("assets/app.js", Member::File(b"console.log(2)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let changed_arg = text_path(&changed);
    let blocked = up.fx.invoke(
        &["--apply", "--dashboard-archive", &changed_arg],
        &[("KILN_MV_FAIL_DASHBOARD", "1")],
    );
    blocked.assert_failed();
    assert!(!up.fx.dashboard_dir().exists());
    assert!(old.is_dir());
    assert_eq!(
        fs::read(old.join("index.html")).unwrap(),
        b"<html>one</html>\n"
    );
    assert!(killed.is_dir());
    assert_eq!(
        fs::read(killed.join("index.html")).unwrap(),
        b"<html>killed</html>\n"
    );
    assert!(!any_named_bytes(
        &share,
        "index.html",
        b"<html>two</html>\n"
    ));
    assert_eq!(fs::read_to_string(share.join("keep")).unwrap(), "keep\n");
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let again = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &changed_arg], &[]);
    again.assert_ok();
    assert_extract_matches(&changed, &up.fx.dashboard_dir());
    assert_eq!(dashboard_asides(&share), Vec::<PathBuf>::new());
    assert_eq!(fs::read_to_string(share.join("keep")).unwrap(), "keep\n");
    up.assert_data_preserved();
}

fn test_rerun_after_killed_dashboard_swap_keeps_files_and_symlinks() {
    let mut up = Upgrade::new();
    up.write_release("0.4.4", X86_64, Some("0.3.0"));
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.stage_app_units();
    let archive = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>one</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let archive_arg = text_path(&archive);
    let first = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &archive_arg], &[]);
    first.assert_ok();
    let share = up.fx.dashboard_dir().parent().unwrap().to_path_buf();
    let old = share.join(".dashboard.oldtree");
    let killed = share.join(".dashboard.killed");
    let notes = share.join(".dashboard.notes");
    let star = share.join(".dashboard.*");
    let outside = up.fx.root.join("outside-target");
    let link = share.join(".dashboard.link");
    let dangling = share.join(".dashboard.dangling");
    fs::rename(up.fx.dashboard_dir(), &old).unwrap();
    fs::create_dir(&killed).unwrap();
    fs::write(killed.join("index.html"), b"<html>killed</html>\n").unwrap();
    fs::write(&notes, "notes\n").unwrap();
    fs::write(&star, "star\n").unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("canary"), "canary\n").unwrap();
    symlink(&outside, &link).unwrap();
    symlink(up.fx.root.join("missing-target"), &dangling).unwrap();
    let changed = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>two</html>\n")),
            ("assets/app.js", Member::File(b"console.log(2)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let changed_arg = text_path(&changed);
    let blocked = up.fx.invoke(
        &["--apply", "--dashboard-archive", &changed_arg],
        &[("KILN_MV_FAIL_DASHBOARD", "1")],
    );
    blocked.assert_failed();
    assert!(!up.fx.dashboard_dir().exists());
    assert!(killed.is_dir());
    assert_eq!(fs::read_to_string(&notes).unwrap(), "notes\n");
    assert_eq!(fs::read_to_string(&star).unwrap(), "star\n");
    assert!(is_symlink(&link));
    assert_eq!(
        fs::read_to_string(outside.join("canary")).unwrap(),
        "canary\n"
    );
    assert!(is_symlink(&dangling));
    up.fx.clear_log();
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let again = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &changed_arg], &[]);
    again.assert_ok();
    assert_extract_matches(&changed, &up.fx.dashboard_dir());
    assert!(!killed.exists());
    assert!(!old.exists());
    assert_eq!(fs::read_to_string(&notes).unwrap(), "notes\n");
    assert_eq!(fs::read_to_string(&star).unwrap(), "star\n");
    assert!(is_symlink(&link));
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(&outside).unwrap()
    );
    assert_eq!(
        fs::read_to_string(outside.join("canary")).unwrap(),
        "canary\n"
    );
    assert!(is_symlink(&dangling));
    assert!(
        up.fx
            .lines(None)
            .iter()
            .any(|line| line == "systemctl enable --now kiln-app-proxy.socket")
    );
    up.assert_data_preserved();
}

fn test_failed_dashboard_restore_removes_the_staged_extract() {
    let mut up = Upgrade::new();
    up.write_release("0.4.4", X86_64, Some("0.3.0"));
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.stage_app_units();
    let archive = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>one</html>\n")),
            ("assets/app.js", Member::File(b"console.log(1)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let archive_arg = text_path(&archive);
    let first = up
        .fx
        .invoke(&["--apply", "--dashboard-archive", &archive_arg], &[]);
    first.assert_ok();
    let changed = up.fx.write_dashboard_archive(
        &[
            ("index.html", Member::File(b"<html>two</html>\n")),
            ("assets/app.js", Member::File(b"console.log(2)\n")),
        ],
        None,
    );
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let changed_arg = text_path(&changed);
    let failed = up.fx.invoke(
        &["--apply", "--dashboard-archive", &changed_arg],
        &[("KILN_MV_FAIL_DASHBOARD", "2")],
    );
    failed.assert_failed();
    let share = up.fx.dashboard_dir().parent().unwrap().to_path_buf();
    let asides = dashboard_asides(&share);
    assert_eq!(asides.len(), 1);
    assert_has(
        &failed.stderr,
        &format!(
            "from {} onto {}",
            asides[0].display(),
            up.fx.dashboard_dir().display()
        ),
    );
    let marker = "staged extract at ";
    assert_has(&failed.stderr, marker);
    let stage = PathBuf::from(
        failed
            .stderr
            .split_once(marker)
            .unwrap()
            .1
            .split_whitespace()
            .next()
            .unwrap(),
    );
    assert!(!stage.exists());
    assert_ne!(stage, asides[0]);
    assert!(!up.fx.dashboard_dir().exists());
    assert_eq!(
        fs::read(asides[0].join("index.html")).unwrap(),
        b"<html>one</html>\n"
    );
    assert!(!any_named_bytes(
        &share,
        "index.html",
        b"<html>two</html>\n"
    ));
    up.assert_data_preserved();
}

fn test_check_only_does_not_mutate_or_stop_services() {
    let up = Upgrade::new();
    let result = up.fx.invoke(&[], &[]);
    result.assert_ok();
    assert_has(&result.stdout, "Check only");
    up.assert_original_binaries();
    up.assert_data_preserved();
    up.assert_untouched_neighbors();
    assert!(!up.opt.join("upgrades").exists());
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
}

fn test_connection_guidance_follows_enrollment_files() {
    let mut up = Upgrade::new();
    up.write_versions("0.4.4", "0.4.4", "", "");
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let enrolled = up.fx.invoke(&[], &[]);
    assert_has(&enrolled.stdout, "kiln login");
    assert_lacks(&enrolled.stdout, "kiln-api enroll");
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    fs::write(up.etc.join("enrollment.pending.json"), "{}\n").unwrap();
    let pending = up.fx.invoke(&[], &[]);
    assert_has(&pending.stdout, "enroll --resume");
    assert_lacks(&pending.stdout, "--url");
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    fs::remove_file(up.etc.join("enrollment.pending.json")).unwrap();
    fs::remove_file(up.etc.join("identity.json")).unwrap();
    let fresh = up.fx.invoke(&[], &[]);
    assert_has(&fresh.stdout, "enroll --url https://100.64.5.6:8443");
}

fn test_busy_operations_restore_gateway_without_stopping_host() {
    let up = Upgrade::new();
    sqlite(&up.journal, "INSERT INTO operations VALUES(1)");
    up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
    let result = up.fx.invoke(&["--apply"], &[]);
    result.assert_failed();
    assert_has(&result.stderr, "restored");
    assert!(
        !up.fx
            .lines(None)
            .iter()
            .any(|line| line == "systemctl stop kiln-host.service")
    );
    assert!(up.fx.lines(Some("sleep ")).len() >= 60);
    up.assert_original_binaries();
    up.assert_untouched_neighbors();
    assert_eq!(
        fs::read_to_string(up.fx.state.join("active-kiln-host.service"))
            .unwrap()
            .trim(),
        "active"
    );
    assert_eq!(
        fs::read_to_string(up.fx.state.join("active-kiln-api.service"))
            .unwrap()
            .trim(),
        "active"
    );
}

fn test_operations_finish_before_host_is_stopped() {
    let up = Upgrade::new();
    sqlite(&up.journal, "INSERT INTO operations VALUES(1)");
    let journal = text_path(&up.journal);
    let result = up
        .fx
        .invoke(&["--apply"], &[("KILN_SLEEP_CLEAR_JOURNAL", &journal)]);
    result.assert_ok();
    let rows = up.fx.lines(None);
    let sleep_at = rows
        .iter()
        .position(|line| line.starts_with("sleep "))
        .unwrap();
    let stop_host = rows
        .iter()
        .position(|line| line == "systemctl stop kiln-host.service")
        .unwrap();
    assert!(sleep_at < stop_host);
    assert_eq!(up.fx.lines(Some("sleep ")).len(), 1);
}

fn test_start_failure_rolls_back_and_signal_retains_the_marker() {
    let up = Upgrade::new();
    fs::write(up.fx.state.join("fail-start-host"), "1\n").unwrap();
    up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
    let failed = up.fx.invoke(&["--apply"], &[]);
    failed.assert_failed();
    assert_has(&failed.stderr, "restored");
    up.assert_original_binaries();
    up.assert_untouched_neighbors();
    assert!(!up.opt.join("pending-upgrade.json").exists());

    fs::write(up.fx.state.join("term-on-stop-host"), "1\n").unwrap();
    up.fx.plan_curl(&["ok"], UPGRADE_CATALOG);
    let interrupted = up.fx.invoke(&["--apply"], &[]);
    interrupted.assert_failed();
    let pending = up.opt.join("pending-upgrade.json");
    assert!(pending.is_file());
    let recovery = json_at(&pending);
    let backup = PathBuf::from(recovery["backup"].as_str().unwrap());
    for (name, data) in &up.original {
        assert_eq!(&fs::read(backup.join(name)).unwrap(), data, "{name}");
    }
    up.assert_untouched_neighbors();
    up.fx.clear_log();
    let retry = up.fx.invoke(&["--apply"], &[]);
    assert_has(&retry.stderr, "pending");
    assert!(up.fx.lines(Some("systemctl ")).is_empty());
}

fn test_readiness_and_partial_copy_roll_back() {
    let up = Upgrade::new();
    up.fx.plan_curl(&["ok", "fail", "ok"], UPGRADE_CATALOG);
    let failed = up.fx.invoke(&["--apply"], &[]);
    assert_has(&failed.stderr, "restored");
    up.assert_original_binaries();
    up.assert_data_preserved();
    up.assert_untouched_neighbors();
    assert!(!up.opt.join("pending-upgrade.json").exists());
    assert_eq!(
        fs::read_to_string(up.fx.state.join("active-kiln-host.service"))
            .unwrap()
            .trim(),
        "active"
    );

    fs::remove_dir_all(up.opt.join("upgrades")).unwrap();
    up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
    let source = text_path(&up.fx.package.join("bin/kiln-api"));
    let copied = up
        .fx
        .invoke(&["--apply"], &[("KILN_CP_FAIL_SOURCE", &source)]);
    assert_has(&copied.stderr, "restored");
    assert!(up.fx.cp_marker.is_file());
    up.assert_original_binaries();
    up.assert_data_preserved();
    up.assert_untouched_neighbors();

    up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
    let replaced = up.fx.invoke(&["--apply"], &[("KILN_MV_FAIL", "1")]);
    replaced.assert_failed();
    assert_has(&replaced.stderr, "restored");
    assert!(up.fx.mv_marker.is_file());
    up.assert_original_binaries();
    up.assert_data_preserved();
    up.assert_untouched_neighbors();
    assert!(!up.opt.join("pending-upgrade.json").exists());
}

fn test_rollback_failure_retains_marker_and_blocks_retry() {
    let up = Upgrade::new();
    up.fx.plan_curl(&["ok", "fail", "fail"], UPGRADE_CATALOG);
    let failed = up.fx.invoke(&["--apply"], &[]);
    assert_has(&failed.stderr, "recovery");
    assert!(up.opt.join("pending-upgrade.json").is_file());
    up.assert_original_binaries();
    up.assert_untouched_neighbors();
    up.fx.clear_log();
    let retry = up.fx.invoke(&["--apply"], &[]);
    assert_has(&retry.stderr, "pending");
    assert!(up.fx.lines(None).is_empty());
}

fn test_bad_address_network_and_killmode_do_not_stop_services() {
    let up = Upgrade::new();
    let cases = [
        (
            vec!["--address", "192.168.1.6"],
            "preserves the existing address",
        ),
        (vec!["--network"], "cannot enable networking"),
        (vec!["--network-uplink", "eth0"], "cannot enable networking"),
    ];
    for (args, message) in cases {
        up.fx.clear_log();
        let mut full = vec!["--apply"];
        full.extend(args);
        let result = up.fx.invoke(&full, &[]);
        assert_has(&result.stderr, message);
        assert!(up.fx.lines(Some("systemctl stop")).is_empty());
    }
    fs::write(up.fx.state.join("killmode"), "control-group\n").unwrap();
    let result = up.fx.invoke(&["--apply"], &[]);
    assert_has(&result.stderr, "KillMode");
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
    up.assert_data_preserved();
    up.assert_untouched_neighbors();
}

fn test_missing_symlink_mixed_and_unhealthy_states_stop_nothing() {
    let mut up = Upgrade::new();
    let path = up.destinations["kiln-api"].clone();
    fs::remove_file(&path).unwrap();
    let missing = up.fx.invoke(&["--apply"], &[]);
    assert_has(&missing.stderr, "missing regular file");
    symlink(up.fx.package.join("bin/kiln-api"), &path).unwrap();
    let linked = up.fx.invoke(&["--apply"], &[]);
    assert_has(&linked.stderr, "missing regular file");
    fs::remove_file(&path).unwrap();
    up.write_versions("0.3.2", "0.4.4", "", "");
    let binary = up.destinations["kiln-api"].clone();
    let rewritten = replace_bytes(&up.original["kiln-api"], b"0.3.2", b"0.3.0");
    fs::write(&binary, rewritten).unwrap();
    set_mode(&binary, 0o755);
    let mixed = up.fx.invoke(&["--apply"], &[]);
    assert_has(&mixed.stderr, "mixed");
    up.write_versions("0.3.2", "0.4.4", "", "");
    fs::write(up.fx.state.join("active-kiln-host.service"), "failed\n").unwrap();
    let unhealthy = up.fx.invoke(&["--apply"], &[]);
    assert_has(&unhealthy.stderr, "healthy");
    fs::write(up.fx.state.join("active-kiln-host.service"), "active\n").unwrap();
    fs::rename(up.var_dir.join("runtime"), up.fx.root.join("saved-runtime")).unwrap();
    let absent = up.fx.invoke(&["--apply"], &[]);
    assert_has(&absent.stderr, "missing installed runtime_dir");
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
    let saved = up
        .fx
        .root
        .join("saved-runtime/snapshots/a1323edbf1db1ff8bef3d5ccd9c6ebb6/memory.snap");
    assert_eq!(fs::read(saved).unwrap(), b"snapshot-marker-not-6gb");
    assert_eq!(fs::read(&up.identity).unwrap(), b"identity-bytes-v1");
    assert_eq!(
        fs::read(&up.identity_rollback).unwrap(),
        b"handmade-rollback-v033"
    );
}

fn test_networked_update_preserves_matching_options() {
    let mut up = Upgrade::new();
    up.write_versions("0.3.0", "0.4.4", "", "");
    let policy = up.etc.join("isolation.json");
    fs::write(&policy, "{\"network\":{\"namespace_scope\":\"kiln\"}}\n").unwrap();
    let refused = up
        .fx
        .invoke(&["--apply", "--network-uplink", "wrong0"], &[]);
    assert_has(&refused.stderr, "uplink");
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
    up.fx.plan_curl(&["ok", "ok"], UPGRADE_CATALOG);
    let updated = up.fx.invoke(
        &[
            "--apply",
            "--address",
            "100.64.5.6",
            "--network",
            "--network-uplink",
            "enp7s0",
        ],
        &[],
    );
    updated.assert_ok();
    assert_eq!(
        fs::read_to_string(&policy).unwrap(),
        "{\"network\":{\"namespace_scope\":\"kiln\"}}\n"
    );
    assert!(
        !up.fx
            .lines(None)
            .iter()
            .any(|line| line == "systemctl stop kiln-network.service")
    );
    up.assert_untouched_neighbors();
}

fn test_unrecognized_installed_version_is_not_blamed_on_the_package() {
    let up = Upgrade::new();
    let text = fs::read_to_string(&up.destinations["kiln"])
        .unwrap()
        .replace("0.3.2", "0.3");
    fs::write(&up.destinations["kiln"], text).unwrap();
    set_mode(&up.destinations["kiln"], 0o755);
    let result = up.fx.invoke(&["--apply"], &[]);
    assert_has(&result.stderr, "unrecognized installed binary: kiln");
    assert_lacks(&result.stderr, "package");
    assert!(up.fx.lines(Some("systemctl stop")).is_empty());
}

fn test_installer_lock_is_exclusive_and_is_not_replaced() {
    let up = Upgrade::new();
    let lock = up.fx.root.join("run/lock/kiln-install.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock)
        .unwrap();
    let inode = file.metadata().unwrap().ino();
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert!(rc == 0, "flock: {}", io::Error::last_os_error());
    let held = up.fx.invoke(&["--address", "8.8.8.8"], &[]);
    assert_has(&held.stderr, "another installer is running");
    assert_eq!(fs::metadata(&lock).unwrap().ino(), inode);
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    assert!(rc == 0, "unlock: {}", io::Error::last_os_error());
    drop(file);
    let released = up.fx.invoke(&["--address", "8.8.8.8"], &[]);
    assert_has(&released.stderr, "private");
    assert_lacks(&released.stderr, "another installer");
    assert_eq!(fs::metadata(&lock).unwrap().ino(), inode);

    set_mode(&lock, 0o666);
    let unsafe_lock = up.fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&unsafe_lock.stderr, "unsafe installer lock");
    assert!(lock.is_file());
    assert_eq!(fs::metadata(&lock).unwrap().ino(), inode);
    fs::remove_file(&lock).unwrap();
    let elsewhere = up.fx.root.join("elsewhere");
    symlink(&elsewhere, &lock).unwrap();
    fs::write(&elsewhere, "nope").unwrap();
    let linked = up.fx.invoke(&["--address", "127.0.0.1"], &[]);
    assert_has(&linked.stderr, "unsafe installer lock");
    assert!(is_symlink(&lock));
}

impl Upgrade {
    fn root_unit(&self, name: &str) -> PathBuf {
        self.fx.root.join("etc/systemd/system").join(name)
    }
}

fn reported(name: &str) -> &str {
    if matches!(name, "kiln-host" | "kiln-api") {
        "kiln-server"
    } else {
        name
    }
}

fn text_path(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

fn set_mode(path: &Path, mode: u32) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms).unwrap();
}

fn mode_bits(path: &Path) -> u32 {
    fs::metadata(path).unwrap().mode() & 0o777
}

fn assert_mode(path: &Path, expected: u32) {
    let mode = mode_bits(path);
    assert_eq!(mode, expected, "{} is {mode:o}", path.display());
}

fn assert_hides_other_writes(path: &Path) {
    let mode = mode_bits(path);
    assert_eq!(mode & 0o002, 0, "{} is {mode:o}", path.display());
}

fn write_executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    set_mode(path, 0o755);
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn assert_has(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "missing {needle:?} in:\n{haystack}"
    );
}

fn assert_lacks(haystack: &str, needle: &str) {
    assert!(
        !haystack.contains(needle),
        "found {needle:?} in:\n{haystack}"
    );
}

fn json_at(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn file_stamp(path: &Path) -> (u64, i128) {
    let meta = fs::metadata(path).unwrap();
    let nanos = meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128;
    (meta.ino(), nanos)
}

fn sqlite(db: &Path, sql: &str) {
    let output = Command::new("sqlite3")
        .arg(db)
        .arg(sql)
        .output()
        .unwrap_or_else(|error| panic!("sqlite3: {error}"));
    assert!(
        output.status.success(),
        "sqlite3 {sql}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn openssl(args: &[&str]) -> Run {
    Run::from_output(
        Command::new("openssl")
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("openssl: {error}")),
    )
}

fn collect_files(root: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() && !is_symlink(&path) {
                stack.push(path);
            } else if path.is_file() {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
}

fn sorted_names() -> Vec<String> {
    let mut names: Vec<String> = NAMES.iter().map(|name| (*name).to_string()).collect();
    names.sort();
    names
}

fn child_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        names.push(entry.unwrap().file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}

fn glob_named(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !dir.is_dir() {
        return found;
    }
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path().join(name);
        if path.exists() || is_symlink(&path) {
            found.push(path);
        }
    }
    found.sort();
    found
}

fn children_starting(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}

fn any_named(dir: &Path, name: &str) -> bool {
    if !dir.exists() {
        return false;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|item| item.to_str()) == Some(name) {
                return true;
            }
            if path.is_dir() && !is_symlink(&path) {
                stack.push(path);
            }
        }
    }
    false
}

fn any_named_bytes(dir: &Path, name: &str, bytes: &[u8]) -> bool {
    if !dir.exists() {
        return false;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|item| item.to_str()) == Some(name)
                && path.is_file()
                && fs::read(&path).unwrap() == bytes
            {
                return true;
            }
            if path.is_dir() && !is_symlink(&path) {
                stack.push(path);
            }
        }
    }
    false
}

fn dashboard_asides(share: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !share.exists() {
        return found;
    }
    for entry in fs::read_dir(share).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".dashboard.")
        {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = data;
    while let Some(pos) = rest.windows(from.len()).position(|window| window == from) {
        out.extend_from_slice(&rest[..pos]);
        out.extend_from_slice(to);
        rest = &rest[pos + from.len()..];
    }
    out.extend_from_slice(rest);
    out
}

fn append_bytes<W: io::Write>(archive: &mut tar::Builder<W>, name: &str, data: &[u8]) {
    let mut header = tar::Header::new_gnu();
    if header.set_path(name).is_err() {
        // Planted `..` members are what the installer must refuse. set_path
        // rejects them, so write the name into the ustar field directly.
        let raw = header.as_old_mut();
        raw.name = [0; 100];
        let bytes = name.as_bytes();
        assert!(bytes.len() < raw.name.len(), "tar path {name} is too long");
        raw.name[..bytes.len()].copy_from_slice(bytes);
    }
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("").unwrap();
    header.set_groupname("").unwrap();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    archive
        .append(&header, Cursor::new(data))
        .unwrap_or_else(|error| panic!("tar append {name}: {error}"));
}

fn append_link<W: io::Write>(archive: &mut tar::Builder<W>, name: &str, target: &str) {
    let mut header = tar::Header::new_gnu();
    header
        .set_path(name)
        .unwrap_or_else(|error| panic!("tar path {name}: {error}"));
    header
        .set_link_name(target)
        .unwrap_or_else(|error| panic!("tar link {target}: {error}"));
    header.set_size(0);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("").unwrap();
    header.set_groupname("").unwrap();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_cksum();
    archive
        .append(&header, io::empty())
        .unwrap_or_else(|error| panic!("tar append link {name}: {error}"));
}

fn assert_extract_matches(archive: &Path, dest: &Path) {
    let members = regular_archive_members(archive);
    let files = regular_tree_files(dest);
    assert_eq!(files, members);
    assert_eq!(
        files.len(),
        members.len(),
        "extracted {} files from {} archive members",
        files.len(),
        members.len()
    );
}

fn regular_archive_members(path: &Path) -> Vec<String> {
    let file = File::open(path).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut names = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        if entry.header().entry_type().is_file() {
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            names.push(name.strip_prefix("./").unwrap_or(&name).to_string());
        }
    }
    names.sort();
    names
}

fn regular_tree_files(dest: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = vec![dest.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() && !is_symlink(&path) {
                stack.push(path);
            } else if path.is_file() {
                names.push(
                    path.strip_prefix(dest)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    names.sort();
    names
}

fn tar_names(path: &Path) -> BTreeSet<String> {
    let file = File::open(path).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    archive
        .entries()
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn tar_bytes(path: &Path, name: &str) -> Vec<u8> {
    let file = File::open(path).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().to_string_lossy() == name {
            let mut stored = Vec::new();
            io::copy(&mut entry, &mut stored).unwrap();
            return stored;
        }
    }
    panic!("missing {name} in {}", path.display());
}

fn log_exit_stub(name: &str, log: &str) -> String {
    format!("#!/bin/bash\nprintf '%s\\n' \"{name} $*\" >> \"{log}\"\nexit 0\n")
}

fn systemctl_stub(log: &str, state: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "systemctl $*" >> "@LOG@"
prop= service=
for arg in "$@"; do
  case "$arg" in
    --property=*) prop=${arg#--property=} ;;
    --value|--no-pager) ;;
    show|stop|start|restart|enable|disable|daemon-reload) ;;
    *) service=$arg ;;
  esac
done
case "$1" in
  show)
    case "$prop" in
      LoadState)
        if [ -f "@STATE@/load-state" ]; then cat "@STATE@/load-state"; else printf '%s\n' not-found; fi
        ;;
      ActiveState) cat "@STATE@/active-$service" ;;
      MainPID)
        if [ -f "@STATE@/pid-$service" ]; then cat "@STATE@/pid-$service"; else printf '%s\n' 0; fi
        ;;
      KillMode) cat "@STATE@/killmode" ;;
      ExecStart) cat "@STATE@/exec-$service" ;;
      *) printf '%s\n' "" ;;
    esac
    ;;
  restart)
    if [ -f "@STATE@/fail-restart-$service" ]; then
      rm -f "@STATE@/fail-restart-$service"
      exit 1
    fi
    old=0
    if [ -f "@STATE@/pid-$service" ]; then
      old=$(tr -d '[:space:]' < "@STATE@/pid-$service")
    fi
    case "$old" in
      ''|*[!0-9]*) old=0 ;;
    esac
    printf '%s\n' "$((old + 1))" > "@STATE@/pid-$service"
    printf '%s\n' active > "@STATE@/active-$service"
    ;;
  stop|start)
    for svc in "${@:2}"; do
      if [ "$1" = stop ]; then printf '%s\n' inactive; else printf '%s\n' active; fi > "@STATE@/active-$svc"
    done
    if [ "$1" = start ] && [ "$2" = kiln-host.service ] && [ -f "@STATE@/fail-start-host" ]; then
      rm -f "@STATE@/fail-start-host"
      exit 1
    fi
    if [ "$1" = stop ] && [ "$2" = kiln-host.service ] && [ -f "@STATE@/term-on-stop-host" ]; then
      rm -f "@STATE@/term-on-stop-host"
      kill -TERM "$PPID"
      sleep 5
      exit 143
    fi
    ;;
  enable)
    if [ -f "@STATE@/fail-enable-socket" ] && [ "$service" = kiln-app-proxy.socket ]; then
      printf '%s\n' 'socket unit is masked' >&2
      exit 1
    fi
    if [ -f "@STATE@/fail-enable" ]; then exit 1; fi
    ;;
  disable|daemon-reload) ;;
  *) exit 1 ;;
esac
"#
    .replace("@LOG@", log)
    .replace("@STATE@", state)
}

fn useradd_stub(log: &str, passwd: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "useradd $*" >> "@LOG@"
name=${!#}
uid=$(id -u)
gid=$(id -g)
prev=
for arg in "$@"; do
  case "$prev" in
    --uid) uid=$arg ;;
    --gid) gid=$arg ;;
  esac
  prev=$arg
done
printf '%s\n' "$name:x:$uid:$gid::/nonexistent:/usr/sbin/nologin" >> "@PASSWD@"
exit 0
"#
    .replace("@LOG@", log)
    .replace("@PASSWD@", passwd)
}

fn curl_stub(log: &str, seq: &str, body: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "curl $*" >> "@LOG@"
if [ ! -s "@SEQ@" ]; then exit 1; fi
line=$(head -n 1 "@SEQ@")
tail -n +2 "@SEQ@" > "@SEQ@.next"
mv "@SEQ@.next" "@SEQ@"
case "$line" in
  ok) cat "@BODY@"; exit 0 ;;
  bad) printf '%s' '{}'; exit 0 ;;
  *) exit 1 ;;
esac
"#
    .replace("@LOG@", log)
    .replace("@SEQ@", seq)
    .replace("@BODY@", body)
}

fn sleep_stub(log: &str, marker: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "sleep $*" >> "@LOG@"
if [ -n "${KILN_SLEEP_CLEAR_JOURNAL:-}" ] && [ ! -f "@MARKER@" ]; then
  touch "@MARKER@"
  sqlite3 "$KILN_SLEEP_CLEAR_JOURNAL" 'UPDATE operations SET running=0;'
fi
exit 0
"#
    .replace("@LOG@", log)
    .replace("@MARKER@", marker)
}

fn cp_stub(log: &str, marker: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "cp $*" >> "@LOG@"
if [ -n "${KILN_CP_FAIL_SOURCE:-}" ] && [ ! -f "@MARKER@" ]; then
  for arg in "$@"; do
    if [ "$arg" = "$KILN_CP_FAIL_SOURCE" ]; then
      touch "@MARKER@"
      exit 1
    fi
  done
fi
exec /bin/cp "$@"
"#
    .replace("@LOG@", log)
    .replace("@MARKER@", marker)
}

fn mv_stub(log: &str, marker: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "mv $*" >> "@LOG@"
if [ -n "${KILN_MV_FAIL:-}" ] && [ ! -f "@MARKER@" ]; then
  live=0
  backup=0
  for arg in "$@"; do
    case "$arg" in
      */.kiln-update-*) live=1 ;;
      */upgrades/*) backup=1 ;;
    esac
  done
  if [ "$live" = 1 ] && [ "$backup" = 0 ]; then
    touch "@MARKER@"
    exit 1
  fi
fi
if [ -n "${KILN_MV_FAIL_DASHBOARD:-}" ]; then
  case "${!#}" in
    */share/kiln/dashboard)
      count=0
      if [ -f "@MARKER@" ]; then
        count=$(cat "@MARKER@")
      fi
      if [ "$count" -lt "$KILN_MV_FAIL_DASHBOARD" ]; then
        printf '%s\n' "$((count + 1))" > "@MARKER@"
        exit 1
      fi
      ;;
  esac
fi
exec /bin/mv "$@"
"#
    .replace("@LOG@", log)
    .replace("@MARKER@", marker)
}

fn runtime_stub(log: &str, template_id: &str) -> String {
    r#"#!/bin/bash
printf '%s\n' "runtime PATH=$PATH" >> "@LOG@"
printf '%s\n' "runtime $*" >> "@LOG@"
case " $* " in
  *" template build "*)
    printf '%s\n' '{"id":"@ID@"}'
    ;;
esac
exit 0
"#
    .replace("@LOG@", log)
    .replace("@ID@", template_id)
}

fn bin_stub(name: &str, log: &str) -> String {
    format!("#!/bin/bash\nprintf '%s\\n' \"bin/{name} $*\" >> \"{log}\"\nexit 0\n")
}

pub fn cli(_args: &[String]) -> i32 {
    crate::run_cases(&cases())
}

register_cases! {
    test_installer_names_binaries_and_keeps_the_production_lock_path,
    test_private_addresses_fail_closed_and_local_bind_is_separate,
    test_supported_hosts_reach_systemd_and_other_hosts_stop_first,
    test_low_memory_stops_before_the_port_bind,
    test_identity_range_and_destination_conflicts_stop_before_install,
    test_network_preflight_rejects_injection_and_missing_nsenter_before_ip,
    test_check_only_does_not_install,
    test_dashboard_archive_extracts_and_enables_the_loopback_unit,
    test_socket_enable_failure_shows_systemctl_stderr,
    test_apply_without_dashboard_archive_leaves_the_share_path_absent,
    test_dashboard_archive_without_apply_writes_nothing,
    test_symlink_dashboard_member_exits_before_the_unit_is_enabled,
    test_dashboard_dotdot_member_is_refused_before_writes,
    test_dashboard_dotdot_child_is_refused_before_writes,
    test_symlink_dashboard_destination_is_refused_before_writes,
    test_symlink_share_ancestor_is_refused_before_writes,
    test_packaged_door_unit_has_no_dashboard_root_and_installer_skips_caddy,
    test_apply_installs_certificates_units_and_network_without_prompt,
    test_failed_readiness_disables_services_and_keeps_state,
    test_update_preserves_configuration_identity_and_snapshot,
    test_upgrade_disables_the_public_door_socket,
    test_dark_forge_layout_upgrades_and_rolls_back_without_release_json,
    test_newer_patch_and_minor_releases_replace_binaries,
    test_downgrade_floor_and_missing_guard_stop_before_any_change,
    test_same_version_replaces_only_different_bytes,
    test_dashboard_extract_replaces_only_different_bytes,
    test_failed_dashboard_replace_keeps_the_installed_tree,
    test_rerun_after_killed_dashboard_swap_clears_asides,
    test_rerun_after_killed_dashboard_swap_keeps_files_and_symlinks,
    test_failed_dashboard_restore_removes_the_staged_extract,
    test_check_only_does_not_mutate_or_stop_services,
    test_connection_guidance_follows_enrollment_files,
    test_busy_operations_restore_gateway_without_stopping_host,
    test_operations_finish_before_host_is_stopped,
    test_start_failure_rolls_back_and_signal_retains_the_marker,
    test_readiness_and_partial_copy_roll_back,
    test_rollback_failure_retains_marker_and_blocks_retry,
    test_bad_address_network_and_killmode_do_not_stop_services,
    test_missing_symlink_mixed_and_unhealthy_states_stop_nothing,
    test_networked_update_preserves_matching_options,
    test_unrecognized_installed_version_is_not_blamed_on_the_package,
    test_installer_lock_is_exclusive_and_is_not_replaced,
}
