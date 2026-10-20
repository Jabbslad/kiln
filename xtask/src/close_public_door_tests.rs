use crate::register_cases;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const PUBLIC: &str = "127.0.0.1:8443";
const BRIDGE: &str = "127.0.0.1:8444";
const PROXYD: &str = "/lib/systemd/systemd-socket-proxyd";

fn test_upgrade_closes_8443_and_keeps_8444() {
    let work = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let stop = Arc::new(AtomicBool::new(false));
    let (backend_port, backend) = start_backend(Arc::clone(&stop));
    let harness = Harness {
        stop,
        backend: Some(backend),
        root: work.path().to_path_buf(),
    };
    let units = work.path().join("units");
    let state = work.path().join("state");
    fs::create_dir_all(&units).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(
        units.join("kiln-door-proxy.service"),
        "Sockets=kiln-door-proxy.socket kiln-door-proxy-public.socket\n",
    )
    .unwrap();
    fs::write(
        units.join("kiln-door-proxy-public.socket"),
        "ListenStream=127.0.0.1:8443\n",
    )
    .unwrap();
    fs::write(state.join("public-active"), "active\n").unwrap();
    fs::write(state.join("proxy-active"), "active\n").unwrap();
    let log = work.path().join("systemctl.log");
    write_systemctl(work.path(), &units, &state, &log, backend_port);
    spawn_proxy(work.path(), &state, &[PUBLIC, BRIDGE], backend_port);
    thread::sleep(Duration::from_millis(300));
    let before = format!("{}\n{}\n{}", listeners(), probe(8443), probe(8444));
    println!("{before}");

    let function = extract_function(
        &fs::read_to_string(crate::repo_root().join("deploy/install.sh")).unwrap(),
        "close_public_door",
    );
    let script = format!("set -euo pipefail\n{function}\nclose_public_door\n");
    let mut command = Command::new("bash");
    command.arg("-c").arg(&script);
    command.env("UNITS", &units);
    command.env("PATH", prepend_path(&work.path().join("bin")));
    let completed = run_timeout(&mut command, Duration::from_secs(15));
    let after = format!("{}\n{}\n{}", listeners(), probe(8443), probe(8444));
    println!("{}\n{}\n{after}", completed.stdout, completed.stderr);
    assert!(completed.status.success(), "{}", completed.stderr);
    let commands = fs::read_to_string(&log).unwrap();
    assert!(commands.contains("systemctl disable --now kiln-door-proxy-public.socket"));
    assert!(!commands.contains("systemctl enable"));
    let fresh = listeners();
    let after_header = fresh.split("State").nth(1).unwrap_or(&fresh);
    assert!(!after_header.contains(":8443"), "{fresh}");
    assert!(fresh.contains(":8444"), "{fresh}");
    assert!(after.contains("port 8443 failed"), "{after}");
    assert!(
        after.contains("port 8444 answered b'door-ok\\n'"),
        "{after}"
    );
    let service =
        fs::read_to_string(crate::repo_root().join("deploy/kiln-door-proxy.service")).unwrap();
    assert!(service.contains("Sockets=kiln-door-proxy.socket\n"));
    assert!(!service.contains("kiln-door-proxy-public.socket"));
    assert!(
        !crate::repo_root()
            .join("deploy/kiln-door-proxy-public.socket")
            .exists()
    );
    drop(harness);
    drop(work);
}

fn start_backend(stop: Arc<AtomicBool>) -> (u16, JoinHandle<()>) {
    let listener =
        TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("bind backend: {error}"));
    let port = listener.local_addr().unwrap().port();
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("backend nonblocking: {error}"));
    let handle = thread::spawn(move || backend_loop(listener, stop));
    (port, handle)
}

fn backend_loop(listener: TcpListener, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.write_all(b"door-ok\n");
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

fn spawn_proxy(work: &Path, state: &Path, addresses: &[&str], backend_port: u16) {
    let mut command = Command::new("systemd-socket-activate");
    for address in addresses {
        command.arg("-l").arg(address);
    }
    command.arg(PROXYD).arg(format!("127.0.0.1:{backend_port}"));
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("systemd-socket-activate: {error}"));
    let pid = child.id().to_string();
    fs::write(state.join("proxy.pid"), &pid).unwrap();
    fs::write(work.join("proxy.pid"), &pid).unwrap();
}

fn write_systemctl(work: &Path, units: &Path, state: &Path, log: &Path, backend_port: u16) {
    let bindir = work.join("bin");
    fs::create_dir_all(&bindir).unwrap();
    let script = r#"#!/bin/bash
printf '%s\n' "systemctl $*" >> __LOG__
cmd=$1
shift
unit=
for arg in "$@"; do
  case "$arg" in
    --*) ;;
    *) unit=$arg ;;
  esac
done
case "$cmd" in
  show)
    if [ -f __UNITS__/kiln-door-proxy-public.socket ]; then
      printf '%s\n' loaded
    else
      printf '%s\n' not-found
    fi
    ;;
  is-active)
    quiet=0
    marker=__STATE__/proxy-active
    for arg in "$@"; do
      case "$arg" in
        --quiet) quiet=1 ;;
        --*) ;;
        *) unit=$arg ;;
      esac
    done
    if [ "$unit" = kiln-door-proxy-public.socket ]; then
      marker=__STATE__/public-active
    fi
    if [ -f "$marker" ]; then
      [ "$quiet" = 1 ] || printf '%s\n' active
      exit 0
    fi
    [ "$quiet" = 1 ] || printf '%s\n' inactive
    exit 3
    ;;
  disable)
    rm -f __STATE__/public-active
    ;;
  daemon-reload) ;;
  restart)
    if [ -f __STATE__/proxy.pid ]; then
      old=$(cat __STATE__/proxy.pid)
      kill "$old" 2>/dev/null || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$old" 2>/dev/null || break
        sleep 0.05
      done
    fi
    args=(systemd-socket-activate)
    if grep -q kiln-door-proxy-public.socket __UNITS__/kiln-door-proxy.service; then
      args+=(-l __PUBLIC__)
    fi
    args+=(-l __BRIDGE__ __PROXYD__ 127.0.0.1:__PORT__)
    setsid "${args[@]}" >/dev/null 2>&1 < /dev/null &
    echo $! > __STATE__/proxy.pid
    echo $! > __WORK__/proxy.pid
    sleep 0.3
    ;;
  *) exit 1 ;;
esac
"#
    .replace("__LOG__", &log.display().to_string())
    .replace("__UNITS__", &units.display().to_string())
    .replace("__STATE__", &state.display().to_string())
    .replace("__WORK__", &work.display().to_string())
    .replace("__PUBLIC__", PUBLIC)
    .replace("__BRIDGE__", BRIDGE)
    .replace("__PROXYD__", PROXYD)
    .replace("__PORT__", &backend_port.to_string());
    let path = bindir.join("systemctl");
    fs::write(&path, script).unwrap();
    set_mode(&path, 0o755);
}

fn listeners() -> String {
    let output = Command::new("ss")
        .arg("-ltn")
        .output()
        .unwrap_or_else(|error| panic!("ss: {error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter(|line| {
            line.starts_with("State") || line.contains(":8443") || line.contains(":8444")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn probe(port: u16) -> String {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_secs(1)) {
        Ok(stream) => stream,
        Err(error) => return format!("port {port} failed {error}"),
    };
    if let Err(error) = stream.set_nonblocking(false) {
        return format!("port {port} failed {error}");
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    if let Err(error) = stream.write_all(b"hi\n") {
        return format!("port {port} failed {error}");
    }
    match read_some(&mut stream, 32) {
        Ok(data) => format!("port {port} answered {}", python_bytes_repr(&data)),
        Err(error) => format!("port {port} failed {error}"),
    }
}

fn read_some(stream: &mut TcpStream, max: usize) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while data.len() < max {
        match stream.read(&mut byte)? {
            0 => break,
            _ => {
                data.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
        }
    }
    Ok(data)
}

fn python_bytes_repr(data: &[u8]) -> String {
    let mut out = String::from("b'");
    for &byte in data {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('\'');
    out
}

fn extract_function(source: &str, name: &str) -> String {
    let pattern = format!(r"(?ms)^{}\(\) \{{.*?^\}}", regex::escape(name));
    let expression =
        regex::Regex::new(&pattern).unwrap_or_else(|error| panic!("function regex: {error}"));
    expression
        .find(source)
        .unwrap_or_else(|| panic!("{name} is missing from deploy/install.sh"))
        .as_str()
        .to_string()
}

struct Completed {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_timeout(command: &mut Command, timeout: Duration) -> Completed {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("bash: {error}"));
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stdout_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let stderr_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("command timed out");
            }
            Ok(None) => thread::sleep(Duration::from_millis(30)),
            Err(error) => panic!("wait: {error}"),
        }
    };
    Completed {
        status,
        stdout: String::from_utf8_lossy(&stdout_thread.join().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_thread.join().unwrap()).into_owned(),
    }
}

fn prepend_path(front: &Path) -> String {
    match std::env::var("PATH") {
        Ok(path) if !path.is_empty() => format!("{}:{path}", front.display()),
        _ => front.display().to_string(),
    }
}

fn set_mode(path: &Path, mode: u32) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}

struct Harness {
    stop: Arc<AtomicBool>,
    backend: Option<JoinHandle<()>>,
    root: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        kill_pid_files(&self.root);
        if let Some(handle) = self.backend.take() {
            let _ = handle.join();
        }
    }
}

fn kill_pid_files(root: &Path) {
    let mut files = Vec::new();
    collect_named(root, "proxy.pid", &mut files);
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(pid) = text.trim().parse::<i32>() else {
            continue;
        };
        kill_pid(pid);
    }
}

fn collect_named(root: &Path, name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_named(&path, name, out);
        } else if path.file_name().and_then(|item| item.to_str()) == Some(name) {
            out.push(path);
        }
    }
}

fn kill_pid(pid: i32) {
    if pid <= 0 {
        return;
    }
    unsafe {
        libc::kill(pid, libc::SIGTERM);
        libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG);
    }
}

pub fn cli(_args: &[String]) -> i32 {
    crate::run_cases(&cases())
}

register_cases! {
    test_upgrade_closes_8443_and_keeps_8444,
}
