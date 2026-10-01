//! Admission policy for the pinned Ubuntu image, not an arbitrary guest sanitizer.
use crate::{Initializer, SystemInitializer};
use box_protocol::{InitializeRequest, PREPARATION_MACHINE_ID};
use std::{
    collections::BTreeMap,
    fs, io,
    os::fd::AsRawFd,
    path::Path,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

const DAEMONS: &[&str] = &[
    "systemd-journald.service",
    "systemd-udevd.service",
    "dbus.service",
];

pub fn preparation_requested(cmdline: &str, machine_id: &str) -> bool {
    cmdline.split_whitespace().any(|arg| arg == "boxd.warm=1")
        && machine_id.trim() == PREPARATION_MACHINE_ID
}

const COMPLETED: &[&str] = &[
    "keyboard-setup.service",
    "ldconfig.service",
    "setvtrgb.service",
    "systemd-binfmt.service",
    "systemd-journal-catalog-update.service",
    "systemd-journal-flush.service",
    "systemd-modules-load.service",
    "systemd-remount-fs.service",
    "systemd-sysctl.service",
    "systemd-sysusers.service",
    "systemd-tmpfiles-setup-dev-early.service",
    "systemd-tmpfiles-setup-dev.service",
    "systemd-tmpfiles-setup.service",
    "systemd-udev-trigger.service",
    "systemd-update-done.service",
    "systemd-update-utmp.service",
];
const ACTIVATORS: &[&str] = &[
    "proc-sys-fs-binfmt_misc.automount",
    "systemd-ask-password-console.path",
    "dbus.socket",
    "snapd.socket",
    "systemd-initctl.socket",
    "systemd-journald-dev-log.socket",
    "systemd-journald.socket",
    "systemd-sysext.socket",
    "systemd-udevd-control.socket",
    "systemd-udevd-kernel.socket",
    "apt-daily-upgrade.timer",
    "apt-daily.timer",
    "dpkg-db-backup.timer",
    "e2scrub_all.timer",
    "fstrim.timer",
    "motd-news.timer",
    "systemd-tmpfiles-clean.timer",
];

fn services(inventory: &str, quiescent: bool) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for block in inventory.trim().split("\n\n") {
        let mut fields = std::collections::BTreeMap::new();
        for line in block.lines() {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| io::Error::other("malformed service inventory"))?;
            if fields.insert(key, value).is_some() {
                return Err(io::Error::other("duplicate service property"));
            }
        }
        let get = |key| {
            fields
                .get(key)
                .copied()
                .ok_or_else(|| io::Error::other(format!("missing {key}")))
        };
        let name = get("Id")?;
        let state = get("ActiveState")?;
        let sub = get("SubState")?;
        let fds: u32 = get("NFileDescriptorStore")?
            .parse()
            .map_err(io::Error::other)?;
        let allowed = match (state, sub) {
            ("inactive", "dead") => true,
            ("active", "exited") => COMPLETED.contains(&name),
            ("active", "running") => {
                name == "box-bootstrap.service" || (!quiescent && DAEMONS.contains(&name))
            }
            _ => false,
        };
        if !allowed || (fds != 0 && (quiescent || name != "systemd-journald.service")) {
            return Err(io::Error::other(format!(
                "unsafe template service: {name} {state}/{sub}, {fds} stored FDs"
            )));
        }
        if !name.ends_with(".service") || names.iter().any(|n| n == name) {
            return Err(io::Error::other("invalid service identity"));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn activators(inventory: &str) -> io::Result<Vec<String>> {
    inventory
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 4
                || !ACTIVATORS.contains(&fields[0])
                || fields[1] != "loaded"
                || fields[2] != "active"
            {
                return Err(io::Error::other(format!(
                    "unsafe template activator: {line}"
                )));
            }
            Ok(fields[0].to_owned())
        })
        .collect()
}

fn credentials(output: &str) -> io::Result<()> {
    if output.lines().collect::<Vec<_>>() != ["a(say) 0", "a(say) 0", "a(ss) 0", "a(ss) 0"] {
        return Err(io::Error::other(
            "template service contains credentials or has an unsupported credential schema",
        ));
    }
    Ok(())
}

fn quiet(jobs: &str, environment: &str) -> io::Result<()> {
    if !jobs.trim().is_empty()
        || environment.lines().any(|line| {
            !matches!(
                line,
                "LANG=C.UTF-8" | "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/snap/bin"
            )
        })
    {
        return Err(io::Error::other(
            "pending jobs or unexpected manager environment",
        ));
    }
    Ok(())
}

fn kernel_thread(stat: &str) -> io::Result<bool> {
    let fields = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("invalid proc stat"))?
        .1;
    let flags: u64 = fields
        .split_whitespace()
        .nth(6)
        .ok_or_else(|| io::Error::other("missing process flags"))?
        .parse()
        .map_err(io::Error::other)?;
    Ok(flags & 0x0020_0000 != 0) // PF_KTHREAD, not an empty argv or a process-name heuristic.
}

fn run(program: &str, args: &[&str]) -> io::Result<String> {
    let output = Command::new("/usr/bin/timeout")
        .args(["--kill-after=1s", "5s", program])
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LANG", "C.UTF-8")
        .env("SYSTEMD_COLORS", "0")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

fn systemctl(args: &[&str]) -> io::Result<String> {
    run("/usr/bin/systemctl", args)
}

fn inventory(quiescent: bool) -> io::Result<Vec<String>> {
    services(
        &systemctl(&[
            "show",
            "--all",
            "--property=Id,ActiveState,SubState,NFileDescriptorStore",
            "*.service",
        ])?,
        quiescent,
    )
}

fn active_sources() -> io::Result<Vec<String>> {
    activators(&systemctl(&[
        "list-units",
        "--no-legend",
        "--plain",
        "--state=active,activating,deactivating",
        "--type=socket,timer,path,automount",
    ])?)
}

fn unit_command(options: &[&str], units: &[String]) -> io::Result<()> {
    if units.is_empty() {
        return Ok(());
    }
    let mut args = options.to_vec();
    args.extend(units.iter().map(String::as_str));
    systemctl(&args).map(|_| ())
}

fn audit_quiet() -> io::Result<()> {
    inventory(true)?;
    if !active_sources()?.is_empty() {
        return Err(io::Error::other("template activation sources still active"));
    }
    quiet(
        &systemctl(&["list-jobs", "--no-legend", "--plain"])?,
        &systemctl(&["show-environment"])?,
    )?;
    for directory in [
        "/run/credentials",
        "/run/systemd/credentials",
        "/run/credstore",
        "/run/credstore.encrypted",
        "/etc/credstore",
        "/etc/credstore.encrypted",
    ] {
        match fs::read_dir(directory) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(io::Error::other(format!("nonempty {directory}")));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == 1 || pid == std::process::id() {
            continue;
        }
        let stat = match fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !kernel_thread(&stat)? {
            return Err(io::Error::other(format!(
                "unexpected template process {pid}"
            )));
        }
    }
    Ok(())
}

struct WarmInitializer {
    sources: Vec<String>,
    retain_pid1: bool,
}

fn manager_refresh(retain_pid1: bool, machine_id: &str) -> io::Result<&'static str> {
    if retain_pid1 {
        if machine_id != PREPARATION_MACHINE_ID {
            return Err(io::Error::other(
                "retained PID1 requires the shared preparation identity",
            ));
        }
        Ok("daemon-reload")
    } else {
        Ok("daemon-reexec")
    }
}

fn measure<T>(
    phases: &mut BTreeMap<&'static str, f64>,
    name: &'static str,
    action: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let start = Instant::now();
    let result = action();
    phases.insert(name, start.elapsed().as_secs_f64() * 1000.);
    result
}

impl WarmInitializer {
    fn prepare() -> io::Result<Self> {
        // Refuse accidental execution of the guest binary on the host, before
        // running any systemctl command that could change services.
        let cmdline = fs::read_to_string("/proc/cmdline")?;
        if !preparation_requested(&cmdline, &fs::read_to_string("/etc/machine-id")?) {
            return Err(io::Error::other(
                "warm preparation requires a pristine warm-mode guest",
            ));
        }
        if !run("/usr/lib/systemd/systemd", &["--version"])?
            .starts_with("systemd 255 (255.4-1ubuntu8.17)")
        {
            return Err(io::Error::other(
                "warm policy requires the audited systemd package",
            ));
        }
        if Path::new(crate::bootstrap::MARKER).exists() {
            return Err(io::Error::other(
                "warm preparation cannot reuse an initialized boot",
            ));
        }
        systemctl(&["start", "dbus.service"])?;
        let names = inventory(false)?;
        for name in names {
            let mut path = String::from("/org/freedesktop/systemd1/unit/");
            for byte in name.bytes() {
                if byte.is_ascii_alphanumeric() {
                    path.push(byte as char);
                } else {
                    path.push_str(&format!("_{byte:02x}"));
                }
            }
            credentials(&run(
                "/usr/bin/busctl",
                &[
                    "get-property",
                    "org.freedesktop.systemd1",
                    &path,
                    "org.freedesktop.systemd1.Service",
                    "SetCredential",
                    "SetCredentialEncrypted",
                    "LoadCredential",
                    "LoadCredentialEncrypted",
                ],
            )?)?;
        }
        let sources = active_sources()?;
        // Gate first: stopping a daemon alone can immediately reactivate it.
        unit_command(&["mask", "--runtime"], &sources)?;
        unit_command(&["stop"], &sources)?;
        let daemons: Vec<_> = DAEMONS.iter().map(|s| s.to_string()).collect();
        unit_command(&["mask", "--runtime"], &daemons)?;
        unit_command(&["stop"], &daemons)?;
        // A full stop (not restart) drops journald's restart-preserved FD store.
        // Verify it; do not silently clear an unexpected retained store.
        audit_quiet()?;
        // Retest after a delay to catch activation races before exposing vsock.
        std::thread::sleep(Duration::from_millis(100));
        audit_quiet()?;
        // Finish boot's filesystem work once, rather than cloning pending
        // kernel writeback and making every restored box repeat it.
        let root = fs::File::open("/")?;
        // SAFETY: root holds an open descriptor on the guest root filesystem.
        if unsafe { libc::syncfs(root.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            sources,
            retain_pid1: cmdline
                .split_whitespace()
                .any(|arg| arg == "boxd.retain_pid1=1"),
        })
    }

    fn reset(&self, request: &InitializeRequest) -> io::Result<()> {
        let start = Instant::now();
        let mut phases = BTreeMap::new();
        let result = (|| {
            // Reject an inconsistent identity before changing anything in the guest.
            let refresh = manager_refresh(self.retain_pid1, &request.machine_id)?;
            measure(&mut phases, "audit", audit_quiet)?;
            measure(&mut phases, "provision", || {
                SystemInitializer
                    .initialize(request)
                    .map_err(io::Error::other)
            })?;
            let mut restart = self.sources.clone();
            restart.extend(DAEMONS.iter().map(|s| s.to_string()));
            // Entropy and identity are provisioned before any activation is enabled.
            // A retained manager must still reload the changed unit masks.
            measure(&mut phases, "unmask", || {
                unit_command(&["unmask", "--runtime", "--no-reload"], &restart)
            })?;
            measure(&mut phases, "manager_refresh", || systemctl(&[refresh]))?;
            measure(&mut phases, "start_services", || {
                unit_command(&["start"], &restart)
            })?;
            measure(&mut phases, "verify_identity", || {
                for destination in ["org.freedesktop.systemd1", "org.freedesktop.DBus"] {
                    let output = run(
                        "/usr/bin/busctl",
                        &[
                            "call",
                            destination,
                            "/",
                            "org.freedesktop.DBus.Peer",
                            "GetMachineId",
                        ],
                    )?;
                    if output != format!("s \"{}\"\n", request.machine_id) {
                        return Err(io::Error::other(format!(
                            "{destination} has an unexpected machine identity"
                        )));
                    }
                }
                Ok(())
            })
        })();
        // Diagnostic only: no entropy, credentials or user-supplied values.
        // Emitted once, outside the measured subphases, on failure as well.
        eprintln!(
            "boxd_warm_reset {}",
            serde_json::json!({
                "schema_version": 1, "retain_pid1": self.retain_pid1,
                "success": result.is_ok(), "phases_ms": phases,
                "total_ms": start.elapsed().as_secs_f64() * 1000.
            })
        );
        result
    }
}

impl Initializer for WarmInitializer {
    fn initialize(&self, request: &InitializeRequest) -> Result<(), String> {
        self.reset(request).map_err(|e| e.to_string())
    }
}

pub async fn serve() -> io::Result<()> {
    let initializer = WarmInitializer::prepare()?;
    crate::bootstrap::serve_with(Arc::new(initializer)).await?;
    // A separate unit invocation gives the workload agent fresh runtime state,
    // environment and invocation ID. The bootstrap never executes workloads.
    systemctl(&["start", "--no-block", "box-guest.service"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_manager_rejects_new_identity_but_default_reexecutes() {
        let fresh = "37aabbee1234567890abcdef98765432";
        assert_eq!(manager_refresh(false, fresh).unwrap(), "daemon-reexec");
        assert_eq!(
            manager_refresh(true, "11111111111111111111111111111111").unwrap(),
            "daemon-reload"
        );
        assert!(manager_refresh(true, fresh).is_err());
    }

    #[test]
    fn reset_measurement_records_failed_step_and_preserves_error() {
        let mut phases = std::collections::BTreeMap::new();
        let error = measure(&mut phases, "failed", || {
            Err::<(), _>(io::Error::other("injected"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "injected");
        assert_eq!(phases.len(), 1);
        assert!(phases["failed"] >= 0.);
        assert_eq!(measure(&mut phases, "passed", || Ok(19)).unwrap(), 19);
        assert_eq!(phases.len(), 2);
    }

    #[test]
    fn preparation_requires_explicit_guest_boot_and_pristine_identity() {
        let pristine = "11111111111111111111111111111111\n";
        assert!(preparation_requested("quiet boxd.warm=1", pristine));
        assert!(!preparation_requested("quiet", pristine));
        assert!(!preparation_requested("not-boxd.warm=1", pristine));
        assert!(!preparation_requested(
            "boxd.warm=1",
            "1234567890abcdef1234567890abcdef\n"
        ));
    }

    #[test]
    fn quiescence_rejects_jobs_environment_and_userspace_processes() {
        let env =
            "LANG=C.UTF-8\nPATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/snap/bin\n";
        assert!(quiet("", env).is_ok());
        assert!(quiet("19 dbus.service start running", env).is_err());
        assert!(quiet("", &format!("{env}SECRET=retained\n")).is_err());
        assert!(!kernel_thread("44 (odd) process) S 1 44 44 0 -1 4194304 0").unwrap());
        assert!(kernel_thread("2 (kthreadd) S 0 0 0 0 -1 2097152 0").unwrap());
        assert!(kernel_thread("invalid").is_err());
    }

    fn service(name: &str, state: &str, sub: &str, fds: &str) -> String {
        format!("Id={name}\nActiveState={state}\nSubState={sub}\nNFileDescriptorStore={fds}\n")
    }

    #[test]
    fn admission_rejects_unknown_services_and_retained_descriptors() {
        let exited = service("systemd-sysctl.service", "active", "exited", "0");
        assert_eq!(services(&exited, true).unwrap(), ["systemd-sysctl.service"]);
        for invalid in [
            service("secret.service", "active", "exited", "0"),
            service("secret.service", "inactive", "dead", "1"),
            service("systemd-sysctl.service", "active", "exited", "2"),
            service("systemd-journald.service", "active", "running", "0"),
            exited.replace("NFileDescriptorStore=0\n", ""),
            exited.replace("NFileDescriptorStore=0", "NFileDescriptorStore=garbage"),
            exited.replace("SubState=exited", "SubState=running"),
        ] {
            assert!(services(&invalid, true).is_err(), "admitted {invalid}");
        }
        assert!(
            services(
                &service("systemd-journald.service", "active", "running", "2"),
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn admission_rejects_unknown_activators_and_nonempty_credentials() {
        assert_eq!(
            activators("dbus.socket loaded active listening D-Bus\n").unwrap(),
            ["dbus.socket"]
        );
        assert!(activators("secret.automount loaded active waiting Secret\n").is_err());
        assert!(activators("dbus.socket loaded activating start D-Bus\n").is_err());
        assert!(credentials("a(say) 0\na(say) 0\na(ss) 0\na(ss) 0\n").is_ok());
        for invalid in [
            "a(say) 1 password 1 23\na(say) 0\na(ss) 0\na(ss) 0\n",
            "a(say) 0\na(say) 0\na(ss) 1 secret /run/secret\na(ss) 0\n",
            "",
            "[unprintable]",
        ] {
            assert!(credentials(invalid).is_err());
        }
    }
}
