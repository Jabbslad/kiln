use crate::register_cases;
use crate::repo_root;
use std::fs::{self, File};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PIN: &str = "172.18.0.3";
const GATEWAY: &str = "172.18.0.1";
const CA_TARGET: &str = "/etc/kiln/ca.crt";
const IMAGE: &str = "caddy:2.10.2";
const REFUSAL: &str = "no configured subnet contains IP address 172.18.0.3";
const APP_SITE: &str = "\
app.dark-forge.dev {\n\
\theader {\n\
\t\tX-Content-Type-Options \"nosniff\"\n\
\t\tX-Robots-Tag \"noindex, nofollow\"\n\
\t}\n\
\trequest_body {\n\
\t\tmax_size 80KB\n\
\t}\n\
\treverse_proxy 172.18.0.1:8093 {\n\
\t\theader_up X-Forwarded-For {http.request.remote.host}\n\
\t\ttransport http {\n\
\t\t\tdial_timeout 3s\n\
\t\t\tresponse_header_timeout 75s\n\
\t\t}\n\
\t}\n\
}\n\
";

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < data.len() {
        if data[index..].starts_with(from) {
            out.extend_from_slice(to);
            index += from.len();
        } else {
            out.push(data[index]);
            index += 1;
        }
    }
    out
}

fn root() -> PathBuf {
    repo_root()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"))
}

fn before(text: &str, earlier: &str, later: &str) {
    assert!(
        position(text, earlier) < position(text, later),
        "{earlier} is not before {later}"
    );
}

fn fenced_sh(marker: &str) -> String {
    let runbook = read(&root().join("docs/runbooks/2026-10-20-app-dark-forge-caddy.md"));
    let mut cursor = 0;
    while let Some(start) = runbook[cursor..].find("```sh\n") {
        let start = cursor + start;
        let end = runbook[start..]
            .find("\n```")
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("unclosed fence"));
        let block = &runbook[start + "```sh\n".len()..end];
        if block.contains(marker) {
            return block.to_string();
        }
        cursor = end + 4;
    }
    panic!("no fenced shell block contains {marker}");
}

fn host_paths(block: &str) -> String {
    block
        .replace("/opt/paseo/Caddyfile.bak-app-dark-forge", "\"$backup\"")
        .replace("/opt/paseo/Caddyfile", "\"$caddyfile\"")
}

fn append_site(live: &Path, site: &Path) {
    let block = host_paths(&fenced_sh("printf '\\n' >> /opt/paseo/Caddyfile"))
        .replace("deploy/app.Caddyfile", "\"$site\"");
    let script = format!("caddyfile=$1\nsite=$2\n{block}");
    let completed = Command::new("bash")
        .args(["-c", &script, "append-site"])
        .arg(live)
        .arg(site)
        .output()
        .unwrap();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
}

fn has_bytes(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|window| window == needle)
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn dns_check(app_rows: &str, apex_rows: &str, validate_exit: i32) -> (bool, bool) {
    let root = tempfile::tempdir().unwrap();
    let answers = root.path().join("answers");
    fs::create_dir(&answers).unwrap();
    fs::write(answers.join("app.dark-forge.dev"), app_rows).unwrap();
    fs::write(answers.join("dark-forge.dev"), apex_rows).unwrap();
    let bindir = root.path().join("bin");
    fs::create_dir(&bindir).unwrap();
    let log = root.path().join("docker.log");
    executable(
        &bindir.join("getent"),
        "#!/bin/bash\ncat \"$GETENT_DIR/$2\"\n",
    );
    executable(
        &bindir.join("docker"),
        &format!(
            "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"$DOCKER_LOG\"\nprintf '%s\\n' 'Valid configuration'\nif [ \"$4\" = validate ]; then\n  exit {validate_exit}\nfi\nexit 0\n"
        ),
    );
    let live = root.path().join("Caddyfile");
    let backup = root.path().join("Caddyfile.bak");
    fs::write(&live, "edited\n").unwrap();
    fs::write(&backup, "backup\n").unwrap();
    let block = host_paths(&fenced_sh("getent ahostsv4 app.dark-forge.dev"));
    let script = format!("caddyfile=$1\nbackup=$2\n{block}");
    let mut env_path = std::env::var("PATH").unwrap_or_default();
    env_path = format!("{}:{env_path}", bindir.display());
    let completed = Command::new("bash")
        .args(["-c", &script, "dns"])
        .arg(&live)
        .arg(&backup)
        .env("PATH", env_path)
        .env("GETENT_DIR", &answers)
        .env("DOCKER_LOG", &log)
        .output()
        .unwrap();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    let restored = fs::read_to_string(&live).unwrap() == "backup\n";
    let reloaded = fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("reload");
    (restored, reloaded)
}

fn two_ports() -> (u16, u16) {
    let admin = TcpListener::bind("127.0.0.1:0").unwrap();
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    (
        admin.local_addr().unwrap().port(),
        http.local_addr().unwrap().port(),
    )
}

fn curl_result(url: &str) -> std::process::Output {
    Command::new("curl")
        .args(["-fsS", "--max-time", "2", url])
        .output()
        .unwrap()
}

fn curl_body(url: &str) -> String {
    let completed = curl_result(url);
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    String::from_utf8_lossy(&completed.stdout).into_owned()
}

fn wait_http(url: &str, body: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        let completed = curl_result(url);
        let stdout = String::from_utf8_lossy(&completed.stdout);
        if completed.status.success() && stdout == body {
            return;
        }
        last = String::from_utf8_lossy(&completed.stderr).into_owned();
        if last.is_empty() {
            last = stdout.into_owned();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{last}");
}

#[allow(clippy::too_many_arguments)]
fn rollback(
    root: &Path,
    live: &Path,
    backup: &Path,
    admin: &str,
    real: &str,
    home: &Path,
    data: &Path,
    config: &Path,
) -> (String, String) {
    let bindir = root.join("bin");
    fs::create_dir_all(&bindir).unwrap();
    let log = root.join("docker.log");
    fs::write(&log, "").unwrap();
    executable(
        &bindir.join("docker"),
        &format!(
            "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"$DOCKER_LOG\"\naction=$4\nif [ \"$action\" = validate ]; then\n  exec {real} validate --config \"$CADDYFILE\" --adapter caddyfile\nfi\nif [ \"$action\" = reload ]; then\n  exec {real} reload --config \"$CADDYFILE\" --adapter caddyfile --address \"$CADDY_ADMIN\"\nfi\nprintf 'unexpected docker %s\\n' \"$*\" >&2\nexit 99\n"
        ),
    );
    let fenced = fenced_sh("if [ \"$rollback\" -eq 1 ]; then");
    let marker = "if [ \"$rollback\" -eq 1 ]; then";
    let block = host_paths(&fenced);
    let block = &block[block.find(marker).expect("rollback marker")..];
    let script = format!("rollback=1\ncaddyfile=$1\nbackup=$2\n{block}");
    let mut path = std::env::var("PATH").unwrap_or_default();
    path = format!("{}:{path}", bindir.display());
    let completed = Command::new("bash")
        .args(["-c", &script, "rollback"])
        .arg(live)
        .arg(backup)
        .env("PATH", path)
        .env("DOCKER_LOG", &log)
        .env("CADDYFILE", live)
        .env("CADDY_ADMIN", admin)
        .env("HOME", home)
        .env("XDG_DATA_HOME", data)
        .env("XDG_CONFIG_HOME", config)
        .output()
        .unwrap();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    (
        String::from_utf8_lossy(&completed.stdout).into_owned(),
        fs::read_to_string(&log).unwrap_or_default(),
    )
}

fn bind_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.starts_with("      - type:") {
            if let Some(block) = current.take() {
                blocks.push(block.join("\n"));
            }
            current = Some(vec![line]);
        } else if current.is_some()
            && (line.starts_with("      - ") || (!line.is_empty() && !line.starts_with(' ')))
        {
            blocks.push(current.take().unwrap().join("\n"));
        } else if let Some(block) = current.as_mut() {
            block.push(line);
        }
    }
    if let Some(block) = current {
        blocks.push(block.join("\n"));
    }
    blocks
}

fn bind_target(text: &str, target: &str) -> Option<String> {
    bind_blocks(text)
        .into_iter()
        .find(|block| format!("{block}\n").contains(&format!("target: {target}\n")))
}

fn docker_enabled() -> bool {
    std::env::var("KILN_CADDY_DOCKER").ok().as_deref() == Some("1")
}

fn docker_base() -> Vec<String> {
    for base in [
        vec!["docker".to_string()],
        vec!["sudo".into(), "docker".into()],
    ] {
        let status = Command::new(&base[0])
            .args(base.iter().skip(1))
            .arg("info")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.map(|status| status.success()).unwrap_or(false) {
            return base;
        }
    }
    panic!("docker is not available");
}

fn output(base: &[String], args: &[&str]) -> String {
    let completed = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(args)
        .output()
        .unwrap();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    String::from_utf8_lossy(&completed.stdout).into_owned()
}

fn test_pinned_address_matches_the_door_socket() {
    let socket_text = read(&root().join("deploy/kiln-door-proxy.socket"));
    let override_text = read(&root().join("deploy/paseo-caddy.override.yaml"));
    assert!(socket_text.contains(&format!("IPAddressAllow={PIN}/32\n")));
    assert!(socket_text.contains(&format!("ListenStream={GATEWAY}:8444\n")));
    assert!(override_text.contains(&format!("ipv4_address: {PIN}\n")));
    let allow = socket_text
        .split("IPAddressAllow=")
        .nth(1)
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let pinned = override_text
        .split("ipv4_address:")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert_eq!(allow, pinned);
    assert!(!override_text.contains("subnet:"));
    assert!(!override_text.contains("kiln-edge"));
    assert!(!override_text.contains("image:"));
    assert!(!override_text.contains("ports:"));
}

fn test_ca_bind_is_read_only_at_create_and_is_not_the_key_directory() {
    let override_text = read(&root().join("deploy/paseo-caddy.override.yaml"));
    let mount = bind_target(&override_text, CA_TARGET).expect("ca mount");
    assert_eq!(bind_blocks(&override_text).len(), 1);
    assert!(mount.contains("source: ${KILN_CA_CERT:-/etc/kiln/ca.crt}\n"));
    assert!(mount.contains("read_only: true\n"));
    assert!(mount.contains("create_host_path: false"));
    let source = mount
        .split("source:")
        .nth(1)
        .unwrap()
        .split('\n')
        .next()
        .unwrap()
        .trim();
    assert!(source.ends_with("ca.crt}"));
    assert_ne!(source, "/etc/kiln");
    for secret in ["ca.key", "door.key", "directory.token", "tls.key"] {
        assert!(!override_text.contains(secret), "{secret}");
    }
    assert!(!override_text.contains("kiln-identity"));
    assert!(!override_text.contains("8092"));
    assert!(!override_text.contains("caddy-data"));
    assert!(!override_text.contains("/etc/caddy/Caddyfile"));
}

fn test_door_site_trusts_the_mounted_ca_with_the_current_directive() {
    let door = read(&root().join("deploy/door.Caddyfile"));
    let runbook = read(&root().join("docs/runbooks/2026-10-19-door-public-cert.md"));
    assert!(door.contains("tls_trust_pool file {\n"));
    assert!(door.contains(&format!("pem_file {CA_TARGET}\n")));
    assert!(door.contains(&format!("https://{GATEWAY}:8444")));
    assert!(door.contains("tls_server_name cli.dark-forge.dev\n"));
    assert!(!door.lines().any(|line| line.starts_with("dark-forge.dev")));
    assert!(!door.contains("tls_trusted_ca_certs"));
    assert!(!runbook.contains("tls_trusted_ca_certs /etc/kiln/ca.crt"));
    assert!(runbook.contains("tls_trust_pool file {"));
    assert!(runbook.contains("docker network inspect paseo_default"));
    assert!(runbook.contains("A check that the address is free would always abort."));
    assert!(runbook.contains(REFUSAL));
    assert!(runbook.contains("That needs a separate plan."));
    assert!(runbook.contains("COMPOSE_FILE"));
    assert!(runbook.contains("docker compose stop caddy\n"));
    assert!(runbook.contains("docker compose rm -f caddy\n"));
    assert!(runbook.contains("docker compose up -d --no-deps caddy\n"));
    assert!(!runbook.contains("kiln-door-caddy"));
    assert!(!runbook.contains("caddy-compose.yaml"));
}

fn test_preflight_aborts_when_no_subnet_contains_the_pin() {
    if !docker_enabled() {
        return;
    }
    let base = docker_base();
    let network = "paseo_nosub";
    let name = "kiln-nosub-caddy";
    let work = tempfile::tempdir().unwrap();
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["network", "rm", network])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    output(
        &base,
        &[
            "network",
            "create",
            "--subnet",
            "172.19.0.0/16",
            "--gateway",
            "172.19.0.1",
            network,
        ],
    );
    let compose = work.path().join("compose.yaml");
    fs::write(
        &compose,
        format!(
            "services:\n  caddy:\n    image: {IMAGE}\n    command: [\"sleep\", \"60\"]\n    container_name: {name}\n    restart: unless-stopped\nnetworks:\n  default:\n    name: {network}\n    external: true\n"
        ),
    )
    .unwrap();
    let up = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["compose", "-p", "paseo-nosub", "-f"])
        .arg(&compose)
        .args(["up", "-d"])
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(up.success());
    let before = output(&base, &["inspect", "-f", "{{.Id}} {{.State.Status}}", name]);
    let raw = output(
        &base,
        &[
            "network",
            "inspect",
            network,
            "--format",
            "{{json .IPAM.Config}}",
        ],
    );
    let configs: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).unwrap_or_default();
    let contains = configs.iter().any(|item| {
        item.get("Subnet")
            .and_then(|value| value.as_str())
            .is_some_and(|subnet| subnet_contains(subnet, PIN))
    });
    assert!(!contains);
    let (ok, reason) = preflight(&base, network);
    assert!(!ok);
    assert_eq!(reason, format!("no subnet contains {PIN}"));
    let after = output(&base, &["inspect", "-f", "{{.Id}} {{.State.Status}}", name]);
    assert_eq!(before, after);
    assert!(after.split_whitespace().nth(1) == Some("running"));
    let ca = work.path().join("ca.crt");
    fs::write(&ca, "placeholder\n").unwrap();
    let override_path = root().join("deploy/paseo-caddy.override.yaml");
    let mut command = Command::new(&base[0]);
    command.args(base.iter().skip(1));
    if base[0] == "sudo" {
        command = Command::new("sudo");
        command
            .arg("env")
            .arg(format!("KILN_CA_CERT={}", ca.display()))
            .arg("docker");
    }
    let refused = command
        .args(["compose", "-p", "paseo-nosub", "-f"])
        .arg(&compose)
        .arg("-f")
        .arg(&override_path)
        .args(["up", "-d", "--force-recreate"])
        .env("KILN_CA_CERT", &ca)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains(REFUSAL));
    let still = output(&base, &["inspect", "-f", "{{.Id}} {{.State.Status}}", name]);
    assert_eq!(still, before);
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["compose", "-p", "paseo-nosub", "-f"])
        .arg(&compose)
        .args(["down", "--remove-orphans"])
        .status();
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["rm", "-f", name])
        .status();
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["network", "rm", network])
        .status();
}

fn subnet_contains(subnet: &str, pin: &str) -> bool {
    let Some((network, prefix)) = subnet.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u32>() else {
        return false;
    };
    let Ok(network) = parse_v4(network) else {
        return false;
    };
    let Ok(address) = parse_v4(pin) else {
        return false;
    };
    if prefix > 32 {
        return false;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    network & mask == address & mask
}

fn parse_v4(text: &str) -> Result<u32, ()> {
    let mut parts = text.split('.');
    let mut value = 0u32;
    for _ in 0..4 {
        let part = parts.next().ok_or(())?;
        let octet: u32 = part.parse().map_err(|_| ())?;
        if octet > 255 {
            return Err(());
        }
        value = (value << 8) | octet;
    }
    if parts.next().is_some() {
        return Err(());
    }
    Ok(value)
}

fn test_recreate_keeps_the_address_the_ca_and_the_401() {
    if !docker_enabled() {
        return;
    }
    let base = docker_base();
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("paseo");
    fs::create_dir(&project).unwrap();
    let ca = work.path().join("ca.crt");
    fs::write(&ca, "copied-ca\n").unwrap();
    lost_copy(&base, work.path());
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["rm", "-f", "caddy"])
        .status();
    for volume in ["paseo_caddy_data", "paseo_caddy_config"] {
        let _ = Command::new(&base[0])
            .args(base.iter().skip(1))
            .args(["volume", "rm", volume])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        output(&base, &["volume", "create", volume]);
    }
    output(
        &base,
        &[
            "run",
            "--rm",
            "--entrypoint",
            "sh",
            "-v",
            "paseo_caddy_data:/data",
            "-v",
            "paseo_caddy_config:/config",
            IMAGE,
            "-c",
            "echo data-marker >/data/marker && echo config-marker >/config/marker",
        ],
    );
    let (ca, public, public_key, leaf, leaf_key) = certs(work.path());
    let caddyfile = project.join("Caddyfile");
    fs::write(
        &caddyfile,
        "{\n\tadmin off\n\tauto_https off\n}\nhttp://app.nowcode.com {\n\trespond \"nowcode-stand-in\"\n}\n",
    )
    .unwrap();
    fs::write(
        project.join("compose.yaml"),
        format!(
            "services:\n  caddy:\n    image: {IMAGE}\n    container_name: caddy\n    restart: unless-stopped\n    ports:\n      - \"80:80\"\n      - \"443:443\"\n    volumes:\n      - type: bind\n        source: ./Caddyfile\n        target: /etc/caddy/Caddyfile\n        read_only: true\n        bind:\n          create_host_path: false\n      - type: bind\n        source: {public}\n        target: /etc/caddy/public.crt\n        read_only: true\n        bind:\n          create_host_path: false\n      - type: bind\n        source: {public_key}\n        target: /etc/caddy/public.key\n        read_only: true\n        bind:\n          create_host_path: false\n      - paseo_caddy_data:/data\n      - paseo_caddy_config:/config\nvolumes:\n  paseo_caddy_data:\n    name: paseo_caddy_data\n    external: true\n  paseo_caddy_config:\n    name: paseo_caddy_config\n    external: true\n",
            public = public.display(),
            public_key = public_key.display(),
        ),
    )
    .unwrap();
    fs::write(project.join(".env"), "COMPOSE_PROJECT_NAME=paseo\n").unwrap();
    compose(&base, &project, &["up", "-d"]);
    let first = container_id(&base, &project);
    let (ok, reason) = preflight(&base, "paseo_default");
    assert!(ok, "{reason}");
    let stranger = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args([
            "run",
            "-d",
            "--name",
            "kiln-caddy-stranger",
            "--network",
            "paseo_default",
            "--ip",
            PIN,
            IMAGE,
            "sleep",
            "60",
        ])
        .output()
        .unwrap();
    if stranger.status.success() {
        let (blocked, why) = preflight(&base, "paseo_default");
        assert!(!blocked);
        assert_eq!(why, format!("another container holds {PIN}"));
        assert_eq!(
            output(&base, &["inspect", "-f", "{{.Id}}", "caddy"]).trim(),
            first
        );
        output(&base, &["rm", "-f", "kiln-caddy-stranger"]);
    }
    let (ok, reason) = preflight(&base, "paseo_default");
    assert!(ok, "{reason}");
    let door = read(&root().join("deploy/door.Caddyfile"));
    let (head, rest) = door.split_once('{').unwrap();
    fs::write(
        &caddyfile,
        format!(
            "{{\n\tadmin off\n\tauto_https off\n}}\nhttps://{head}{{\n\ttls /etc/caddy/public.crt /etc/caddy/public.key\n{rest}\nhttp://app.nowcode.com {{\n\trespond \"nowcode-stand-in\"\n}}\n"
        ),
    )
    .unwrap();
    fs::write(
        project.join(".env"),
        format!(
            "COMPOSE_PROJECT_NAME=paseo\nCOMPOSE_FILE=compose.yaml:{}\nKILN_CA_CERT={}\n",
            root().join("deploy/paseo-caddy.override.yaml").display(),
            ca.display()
        ),
    )
    .unwrap();
    compose(
        &base,
        &project,
        &["up", "-d", "--force-recreate", "--no-deps", "caddy"],
    );
    let pinned = container_id(&base, &project);
    assert_container(&base, &pinned, &ca);
    let _standin = StandIn::start(&leaf, &leaf_key);
    assert_eq!(curl_through(), "door-stand-in 401 from 172.18.0.3\n");
    assert_eq!(curl_extra(), "nowcode-stand-in");
    let (status, body) = host_status();
    assert_eq!(status, 403);
    assert!(!body.contains(&format!("from {PIN}")));
    assert_eq!(
        markers(&base, &pinned),
        ("data-marker\n".into(), "config-marker\n".into())
    );
    let rendered = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["compose", "config"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let rendered = String::from_utf8_lossy(&rendered.stdout);
    assert!(rendered.contains(&format!("ipv4_address: {PIN}")));
    assert!(rendered.contains(CA_TARGET));
    compose(&base, &project, &["up", "-d"]);
    let plain = container_id(&base, &project);
    assert_container(&base, &plain, &ca);
    compose(
        &base,
        &project,
        &["up", "-d", "--force-recreate", "--no-deps", "caddy"],
    );
    let again = container_id(&base, &project);
    assert_ne!(pinned, again);
    assert_container(&base, &again, &ca);
    assert_eq!(curl_through(), format!("door-stand-in 401 from {PIN}\n"));
    assert_eq!(curl_extra(), "nowcode-stand-in");
    assert_eq!(
        markers(&base, &again),
        ("data-marker\n".into(), "config-marker\n".into())
    );
    fs::write(
        &caddyfile,
        "{\n\tadmin off\n\tauto_https off\n}\nhttp://app.nowcode.com {\n\trespond \"nowcode-stand-in\"\n}\n",
    )
    .unwrap();
    fs::write(project.join(".env"), "COMPOSE_PROJECT_NAME=paseo\n").unwrap();
    compose(&base, &project, &["stop", "caddy"]);
    compose(&base, &project, &["rm", "-f", "caddy"]);
    compose(&base, &project, &["up", "-d", "--no-deps", "caddy"]);
    let rolled = container_id(&base, &project);
    let status = output(&base, &["inspect", "-f", "{{.State.Status}}", "caddy"]);
    assert_eq!(status.trim(), "running");
    assert_ne!(rolled, again);
    let mounts = output(&base, &["inspect", "-f", "{{json .Mounts}}", &rolled]);
    let mounts: serde_json::Value = serde_json::from_str(&mounts).unwrap();
    let destinations: Vec<&str> = mounts
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.get("Destination").and_then(|value| value.as_str()))
        .collect();
    assert!(!destinations.contains(&CA_TARGET));
    assert_eq!(curl_extra(), "nowcode-stand-in");
    assert_eq!(
        markers(&base, &rolled),
        ("data-marker\n".into(), "config-marker\n".into())
    );
    let door_file = root().join("deploy/door.Caddyfile");
    let mount = format!("{}:/etc/caddy/Caddyfile:ro", door_file.display());
    let adapted = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["run", "--rm", "-v"])
        .arg(&mount)
        .arg(IMAGE)
        .args([
            "caddy",
            "adapt",
            "--config",
            "/etc/caddy/Caddyfile",
            "--adapter",
            "caddyfile",
        ])
        .output()
        .unwrap();
    assert!(
        adapted.status.success(),
        "{}",
        String::from_utf8_lossy(&adapted.stderr)
    );
    let adapted = String::from_utf8_lossy(&adapted.stdout);
    assert!(adapted.contains("\"provider\":\"file\""));
    assert!(adapted.contains(CA_TARGET));
    assert!(!adapted.contains("deprecated"));
    let _ = leaf;
    compose(&base, &project, &["down", "--remove-orphans"]);
    for volume in ["paseo_caddy_data", "paseo_caddy_config"] {
        let _ = Command::new(&base[0])
            .args(base.iter().skip(1))
            .args(["volume", "rm", volume])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn certs(work: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let ca_key = work.join("ca.key");
    let ca = work.join("ca.crt");
    let leaf_key = work.join("standin.key");
    let leaf = work.join("standin.crt");
    let public_key = work.join("public.key");
    let public = work.join("public.crt");
    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=kiln private CA",
            "-keyout",
        ])
        .arg(&ca_key)
        .arg("-out")
        .arg(&ca)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=cli.dark-forge.dev",
            "-keyout",
        ])
        .arg(&leaf_key)
        .arg("-out")
        .arg(work.join("standin.csr"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(
        work.join("standin.ext"),
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:cli.dark-forge.dev\n",
    )
    .unwrap();
    let status = Command::new("openssl")
        .args(["x509", "-req", "-in"])
        .arg(work.join("standin.csr"))
        .args(["-CA"])
        .arg(&ca)
        .args(["-CAkey"])
        .arg(&ca_key)
        .args(["-set_serial", "1", "-days", "2", "-sha256", "-extfile"])
        .arg(work.join("standin.ext"))
        .arg("-out")
        .arg(&leaf)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=cli.dark-forge.dev",
            "-addext",
            "subjectAltName=DNS:cli.dark-forge.dev",
            "-keyout",
        ])
        .arg(&public_key)
        .arg("-out")
        .arg(&public)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    (ca, public, public_key, leaf, leaf_key)
}

fn lost_copy(base: &[String], work: &Path) {
    let source = work.join("copied.crt");
    fs::write(&source, "copied-ca\n").unwrap();
    let name = "kiln-caddy-copy-loss";
    let _ = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["rm", "-f", name])
        .status();
    output(&base, &["run", "-d", "--name", name, IMAGE, "sleep", "30"]);
    let copied = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["cp"])
        .arg(&source)
        .arg(format!("{name}:{CA_TARGET}"))
        .output()
        .unwrap();
    assert!(!copied.status.success());
    output(&base, &["exec", name, "mkdir", "-p", "/etc/kiln"]);
    output_arg(
        base,
        &["cp"],
        &[&source, &PathBuf::from(format!("{name}:{CA_TARGET}"))],
    );
    let inside = output(&base, &["exec", name, "cat", CA_TARGET]);
    assert_eq!(inside, "copied-ca\n");
    output(&base, &["rm", "-f", name]);
    output(&base, &["run", "-d", "--name", name, IMAGE, "sleep", "30"]);
    let missing = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["exec", name, "cat", CA_TARGET])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    output(&base, &["rm", "-f", name]);
}

fn output_arg(base: &[String], prefix: &[&str], paths: &[&Path]) {
    let completed = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(prefix)
        .args(paths.iter().map(|path| path.as_os_str()))
        .output()
        .unwrap();
    assert!(completed.status.success());
}

fn compose(base: &[String], project: &Path, args: &[&str]) {
    let status = Command::new(&base[0])
        .args(base.iter().skip(1))
        .arg("compose")
        .args(args)
        .current_dir(project)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

fn container_id(base: &[String], project: &Path) -> String {
    let raw = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["compose", "ps", "-q"])
        .current_dir(project)
        .output()
        .unwrap();
    assert!(raw.status.success());
    let text = String::from_utf8_lossy(&raw.stdout);
    let id = text.lines().next().unwrap_or("").trim();
    assert!(!id.is_empty());
    id.to_string()
}

fn address(base: &[String], container: &str) -> String {
    output(
        base,
        &[
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            container,
        ],
    )
    .trim()
    .to_string()
}

fn assert_container(base: &[String], container: &str, ca: &Path) {
    assert_eq!(address(base, container), PIN);
    let mounts = output(base, &["exec", container, "cat", "/proc/mounts"]);
    let matched: Vec<_> = mounts
        .lines()
        .filter(|line| line.contains(&format!(" {CA_TARGET} ")))
        .collect();
    assert_eq!(matched.len(), 1, "{mounts}");
    assert!(matched[0].contains(" ro,"));
    let body = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["exec", container, "cat", CA_TARGET])
        .output()
        .unwrap();
    assert_eq!(body.stdout, fs::read(ca).unwrap());
    let listing = output(base, &["exec", container, "ls", "-1", "/etc/kiln"]);
    assert_eq!(listing.split_whitespace().collect::<Vec<_>>(), ["ca.crt"]);
    let wrote = Command::new(&base[0])
        .args(base.iter().skip(1))
        .args(["exec", container, "touch", CA_TARGET])
        .output()
        .unwrap();
    assert!(!wrote.status.success());
    assert!(String::from_utf8_lossy(&wrote.stderr).contains("Read-only file system"));
}

fn markers(base: &[String], container: &str) -> (String, String) {
    (
        output(base, &["exec", container, "cat", "/data/marker"]),
        output(base, &["exec", container, "cat", "/config/marker"]),
    )
}

fn preflight(base: &[String], network: &str) -> (bool, String) {
    let raw = output(
        base,
        &[
            "network",
            "inspect",
            network,
            "--format",
            "{{json .IPAM.Config}}",
        ],
    );
    let configs: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).unwrap_or_default();
    let contains = configs.iter().any(|item| {
        item.get("Subnet")
            .and_then(|value| value.as_str())
            .is_some_and(|subnet| subnet_contains(subnet, PIN))
    });
    if !contains {
        return (false, format!("no subnet contains {PIN}"));
    }
    let inspected = output(base, &["network", "inspect", network]);
    let payload: Vec<serde_json::Value> = serde_json::from_str(&inspected).unwrap();
    let containers = payload
        .first()
        .and_then(|item| item.get("Containers"))
        .and_then(|value| value.as_object());
    if let Some(containers) = containers {
        for (cid, info) in containers {
            let ip = info
                .get("IPv4Address")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .split('/')
                .next()
                .unwrap_or("");
            if ip == PIN && !paseo_caddy(base, cid) {
                return (false, format!("another container holds {PIN}"));
            }
        }
    }
    (true, "ok".into())
}

fn paseo_caddy(base: &[String], cid: &str) -> bool {
    let raw = output(base, &["inspect", "-f", "{{json .Config.Labels}}", cid]);
    let labels: serde_json::Value =
        serde_json::from_str(raw.trim()).unwrap_or(serde_json::json!({}));
    labels
        .get("com.docker.compose.project")
        .and_then(|value| value.as_str())
        == Some("paseo")
        && labels
            .get("com.docker.compose.service")
            .and_then(|value| value.as_str())
            == Some("caddy")
}

struct StandIn {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StandIn {
    fn start(cert: &Path, key: &Path) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let cert = cert.to_path_buf();
        let key = key.to_path_buf();
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || serve_standin(cert, key, flag, sender));
        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("stand-in: {error}"),
            Err(_) => panic!("stand-in exited before listening"),
        }
        let address = format!("{GATEWAY}:8444").parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
                return Self {
                    stop,
                    thread: Some(thread),
                };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&address, Duration::from_secs(1));
        let _ = thread.join();
        panic!("stand-in did not listen");
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let address = format!("{GATEWAY}:8444").parse().unwrap();
        let _ = TcpStream::connect_timeout(&address, Duration::from_secs(1));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy)]
struct SslApi {
    init: unsafe extern "C" fn(u64, *const libc::c_void) -> i32,
    method: unsafe extern "C" fn() -> *const libc::c_void,
    ctx_new: unsafe extern "C" fn(*const libc::c_void) -> *mut libc::c_void,
    use_cert: unsafe extern "C" fn(*mut libc::c_void, *const libc::c_char, i32) -> i32,
    use_key: unsafe extern "C" fn(*mut libc::c_void, *const libc::c_char, i32) -> i32,
    ssl_new: unsafe extern "C" fn(*mut libc::c_void) -> *mut libc::c_void,
    set_fd: unsafe extern "C" fn(*mut libc::c_void, i32) -> i32,
    accept: unsafe extern "C" fn(*mut libc::c_void) -> i32,
    read: unsafe extern "C" fn(*mut libc::c_void, *mut u8, i32) -> i32,
    write: unsafe extern "C" fn(*mut libc::c_void, *const u8, i32) -> i32,
    free: unsafe extern "C" fn(*mut libc::c_void),
    ctx_free: unsafe extern "C" fn(*mut libc::c_void),
}

fn load_symbol<T>(handle: *mut libc::c_void, name: &std::ffi::CStr) -> T {
    unsafe {
        libc::dlerror();
        let symbol = libc::dlsym(handle, name.as_ptr());
        if symbol.is_null() {
            panic!("missing {}", name.to_string_lossy());
        }
        assert_eq!(
            std::mem::size_of::<T>(),
            std::mem::size_of::<*mut libc::c_void>()
        );
        std::mem::transmute_copy(&symbol)
    }
}

fn load_ssl() -> SslApi {
    unsafe {
        let handle = libc::dlopen(c"libssl.so.3".as_ptr(), libc::RTLD_NOW);
        if handle.is_null() {
            panic!("libssl is not available");
        }
        SslApi {
            init: load_symbol(handle, c"OPENSSL_init_ssl"),
            method: load_symbol(handle, c"TLS_server_method"),
            ctx_new: load_symbol(handle, c"SSL_CTX_new"),
            use_cert: load_symbol(handle, c"SSL_CTX_use_certificate_file"),
            use_key: load_symbol(handle, c"SSL_CTX_use_PrivateKey_file"),
            ssl_new: load_symbol(handle, c"SSL_new"),
            set_fd: load_symbol(handle, c"SSL_set_fd"),
            accept: load_symbol(handle, c"SSL_accept"),
            read: load_symbol(handle, c"SSL_read"),
            write: load_symbol(handle, c"SSL_write"),
            free: load_symbol(handle, c"SSL_free"),
            ctx_free: load_symbol(handle, c"SSL_CTX_free"),
        }
    }
}

fn door_response(peer: &str) -> Vec<u8> {
    let code = if peer == PIN { 401 } else { 403 };
    let reason = if code == 401 {
        "Unauthorized"
    } else {
        "Forbidden"
    };
    let body = format!("door-stand-in {code} from {peer}\n");
    let header = format!(
        "HTTP/1.0 {code} {reason}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut bytes = header.into_bytes();
    bytes.extend(body.into_bytes());
    bytes
}

fn handle_tls(api: &SslApi, ctx: *mut libc::c_void, stream: TcpStream) {
    let peer = stream
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_default();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let fd = stream.as_raw_fd();
    unsafe {
        let ssl = (api.ssl_new)(ctx);
        if ssl.is_null() {
            return;
        }
        if (api.set_fd)(ssl, fd) != 1 || (api.accept)(ssl) != 1 {
            (api.free)(ssl);
            return;
        }
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let size = (api.read)(ssl, buffer.as_mut_ptr(), buffer.len() as i32);
            if size <= 0 {
                break;
            }
            request.extend_from_slice(&buffer[..size as usize]);
            if has_bytes(&request, b"\r\n\r\n") || request.len() > 8192 {
                break;
            }
        }
        let response = door_response(&peer);
        let mut sent = 0usize;
        while sent < response.len() {
            let size = (api.write)(
                ssl,
                response[sent..].as_ptr(),
                (response.len() - sent) as i32,
            );
            if size <= 0 {
                break;
            }
            sent += size as usize;
        }
        (api.free)(ssl);
    }
}

fn serve_standin(
    cert: PathBuf,
    key: PathBuf,
    stop: Arc<AtomicBool>,
    sender: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let listener = match TcpListener::bind(format!("{GATEWAY}:8444")) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = sender.send(Err(error.to_string()));
            return;
        }
    };
    let api = load_ssl();
    let cert_c = std::ffi::CString::new(cert.to_string_lossy().as_bytes()).unwrap();
    let key_c = std::ffi::CString::new(key.to_string_lossy().as_bytes()).unwrap();
    let ctx = unsafe {
        (api.init)(0, std::ptr::null());
        let method = (api.method)();
        let ctx = (api.ctx_new)(method);
        if ctx.is_null()
            || (api.use_cert)(ctx, cert_c.as_ptr(), 1) != 1
            || (api.use_key)(ctx, key_c.as_ptr(), 1) != 1
        {
            if !ctx.is_null() {
                (api.ctx_free)(ctx);
            }
            let _ = sender.send(Err("could not load the stand-in certificate".to_string()));
            return;
        }
        ctx
    };
    if sender.send(Ok(())).is_err() {
        unsafe { (api.ctx_free)(ctx) };
        return;
    }
    while !stop.load(Ordering::SeqCst) {
        let accepted = listener.accept();
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok((stream, _)) = accepted else {
            break;
        };
        handle_tls(&api, ctx, stream);
    }
    unsafe { (api.ctx_free)(ctx) };
}

fn curl_through() -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        let completed = Command::new("curl")
            .args([
                "-sk",
                "--resolve",
                "cli.dark-forge.dev:443:127.0.0.1",
                "--max-time",
                "5",
                "https://cli.dark-forge.dev/v1/templates",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&completed.stdout).into_owned();
        if completed.status.success() && stdout.starts_with("door-stand-in ") {
            return stdout;
        }
        last = String::from_utf8_lossy(&completed.stderr).into_owned();
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("{last}");
}

fn curl_extra() -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        let completed = Command::new("curl")
            .args([
                "-s",
                "--resolve",
                "app.nowcode.com:80:127.0.0.1",
                "--max-time",
                "5",
                "http://app.nowcode.com/",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&completed.stdout).into_owned();
        if completed.status.success() && !stdout.is_empty() {
            return stdout;
        }
        last = stdout;
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("{last}");
}

fn host_status() -> (u16, String) {
    let resolve = format!("cli.dark-forge.dev:8444:{GATEWAY}");
    let completed = Command::new("curl")
        .args([
            "-sk",
            "--resolve",
            &resolve,
            "--max-time",
            "5",
            "-w",
            "\n%{http_code}",
            "https://cli.dark-forge.dev:8444/v1/templates",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&completed.stdout);
    let (body, code) = stdout.rsplit_once('\n').unwrap_or((stdout.as_ref(), "0"));
    (code.trim().parse().unwrap_or(0), body.to_string())
}

fn test_site_is_cleartext_and_the_runbook_can_restore_identity() {
    let site = read(&root().join("deploy/app.Caddyfile"));
    let runbook = read(&root().join("docs/runbooks/2026-10-20-app-dark-forge-caddy.md"));
    let unit = read(&root().join("deploy/kiln-identity.service"));
    let binary = "/usr/local/libexec/kiln-identity";
    let database = "/var/lib/kiln-identity/identity.db";
    assert_eq!(site, APP_SITE);
    assert_eq!(site.matches("reverse_proxy ").count(), 1);
    assert!(!site.contains("tls_trust_pool"));
    assert!(!site.contains("8443"));
    assert!(!site.contains("dns-01"));
    assert!(unit.contains(&format!("ExecStart={binary} ")));
    assert!(unit.contains(&format!("--database {database} ")));
    assert!(unit.contains("--signing-key-file "));
    assert!(!runbook.contains("signing.pem"));
    assert!(!runbook.contains("dns-01"));
    assert!(!runbook.contains(".pem"));
    assert!(!runbook.contains(".key"));
    assert!(runbook.contains(binary));
    assert!(runbook.contains(database));
    assert!(runbook.contains("identity.db-wal"));
    assert!(runbook.contains("identity.db-shm"));
    let yes = "Restart `kiln-identity` only after Jamie says yes.";
    assert!(runbook.contains(yes));
    before(&runbook, yes, "systemctl start kiln-identity");
    before(
        &runbook,
        &format!("sha256sum {database}"),
        "systemctl start kiln-identity",
    );
    assert!(runbook.contains("systemctl stop kiln-identity"));
    assert!(runbook.contains("sha256sum -c /var/backups/kiln-identity-app/checksums"));
    assert!(runbook.contains(&format!(
        "install -m 755 /var/backups/kiln-identity-app/kiln-identity {binary}"
    )));
    before(
        &runbook,
        "cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-app-dark-forge",
        "cat deploy/app.Caddyfile >> /opt/paseo/Caddyfile",
    );
    assert!(runbook.contains("caddy validate"));
    assert!(runbook.contains("caddy reload"));
    assert!(runbook.contains("https://dark-forge.dev/login"));
    assert!(runbook.contains("Light the way in."));
    assert!(runbook.contains("https://cli.dark-forge.dev/v1/templates"));
    assert!(runbook.contains("HTTP 401"));
    assert!(runbook.contains("tcp/8443 stays closed."));
    assert!(runbook.contains("Do not add an accept rule."));
    assert!(!runbook.contains("dport 8443 accept"));
    assert!(runbook.contains("127.0.0.1:8093"));
    assert!(runbook.contains("172.18.0.1:8093"));
    assert!(runbook.contains("The browser check is for Kiln Ops after the release."));
    assert!(runbook.contains("This pull request does not run it."));
    assert!(!runbook.contains("This procedure does not start `kiln-app.service`."));
    assert!(!runbook.contains("Stop if that output shows `127.0.0.1:8093`"));
    assert!(runbook.contains("cannot install dashboard archive"));
    assert!(runbook.contains("cannot restore the dashboard"));
    assert!(runbook.contains("The live directory is already back."));
    assert!(runbook.contains("Leave the backup where it is."));
    assert!(!runbook.contains("the live directory is gone"));
    assert!(runbook.contains(
        "cp -a /usr/local/share/kiln/dashboard /var/backups/kiln-dashboard-app/dashboard"
    ));
    before(
        &runbook,
        "cp -a /usr/local/share/kiln/dashboard",
        "bash install.sh --apply --dashboard-archive /var/tmp/kiln-dashboard.tar.gz",
    );
    before(
        &runbook,
        "bash install.sh --apply --dashboard-archive /var/tmp/kiln-dashboard.tar.gz",
        "systemctl restart kiln-app",
    );
    before(
        &runbook,
        "systemctl restart kiln-app",
        "systemctl enable --now kiln-app-proxy.socket",
    );
    before(
        &runbook,
        "systemctl is-active kiln-app",
        "systemctl enable --now kiln-app-proxy.socket",
    );
    assert!(runbook.contains("http://127.0.0.1:8093/v1/x"));
    before(
        &runbook,
        "systemctl enable --now kiln-app-proxy.socket",
        "getent ahostsv4 app.dark-forge.dev",
    );
    assert!(runbook.contains("}app.dark-forge.dev {"));
    assert!(!runbook.contains("grep -q '}app.dark-forge.dev {'"));
    assert!(!runbook.contains("if ! grep -qx 'app.dark-forge.dev {' /opt/paseo/Caddyfile; then"));
    assert!(runbook.contains("line == \"}app.dark-forge.dev {\""));
    assert!(runbook.contains("line == \"app.dark-forge.dev {\""));
    assert!(runbook.contains("If a whole line is `}app.dark-forge.dev {`"));
    assert!(!runbook.contains("already contains `}app.dark-forge.dev {`"));
    let dns_block = fenced_sh("getent ahostsv4 app.dark-forge.dev");
    before(&dns_block, "caddy validate", "caddy reload");
    assert!(runbook.contains("printf '\\n' >> /opt/paseo/Caddyfile"));
    assert!(runbook.contains("awk '{ print $1 }' | sort -u"));
    assert!(runbook.contains("[ -z \"$apex\" ]"));
    assert!(runbook.contains("Restored Caddyfile failed validation. Did not reload."));
    let validate_at = position(
        &runbook,
        "caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile; then",
    );
    let reload_at = runbook
        .rfind("caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile")
        .unwrap();
    assert!(validate_at < reload_at);
    before(
        &runbook,
        "getent ahostsv4 app.dark-forge.dev",
        "docker exec caddy caddy reload",
    );
    assert!(runbook.contains("78.46.98.73"));
    assert!(runbook.contains("https://app.dark-forge.dev/"));
    assert!(runbook.contains("https://app.dark-forge.dev/v1/x"));
    assert!(runbook.contains("\"code\":\"not_found\""));
    assert!(runbook.contains("text/html"));
    before(
        &runbook,
        "https://app.dark-forge.dev/",
        "if [ \"$rollback\" -eq 1 ]; then",
    );
    let rollback_at = position(&runbook, "if [ \"$rollback\" -eq 1 ]; then");
    let restore_at = runbook
        .rfind("cp -a /opt/paseo/Caddyfile.bak-app-dark-forge /opt/paseo/Caddyfile")
        .unwrap();
    assert!(rollback_at < restore_at);
}

fn test_append_repairs_a_missing_newline_and_does_not_repeat() {
    let site = root().join("deploy/app.Caddyfile");
    let tmp = tempfile::tempdir().unwrap();
    let live = tmp.path().join("Caddyfile");
    fs::write(&live, b"example.test {\n\trespond \"ok\"\n}").unwrap();
    let broken = tmp.path().join("broken.Caddyfile");
    let mut bytes = fs::read(&live).unwrap();
    bytes.extend(fs::read(&site).unwrap());
    fs::write(&broken, &bytes).unwrap();
    assert!(has_bytes(
        &fs::read(&broken).unwrap(),
        b"}app.dark-forge.dev {"
    ));
    let rejected = Command::new("caddy")
        .args(["adapt", "--config"])
        .arg(&broken)
        .args(["--adapter", "caddyfile"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("unrecognized directive"));
    append_site(&live, &site);
    append_site(&live, &site);
    let text = fs::read_to_string(&live).unwrap();
    assert_eq!(text.matches("app.dark-forge.dev {").count(), 1);
    assert!(!text.contains("}app.dark-forge.dev"));
    assert!(text.ends_with('\n'));
    let adapted = Command::new("caddy")
        .args(["adapt", "--config"])
        .arg(&live)
        .args(["--adapter", "caddyfile"])
        .output()
        .unwrap();
    assert!(
        adapted.status.success(),
        "{}",
        String::from_utf8_lossy(&adapted.stderr)
    );
}

fn test_append_repairs_a_glued_site() {
    let site = root().join("deploy/app.Caddyfile");
    let tmp = tempfile::tempdir().unwrap();
    let live = tmp.path().join("Caddyfile");
    let mut bytes = b"example.test {\n\trespond \"ok\"\n}".to_vec();
    bytes.extend(fs::read(&site).unwrap());
    fs::write(&live, &bytes).unwrap();
    assert!(has_bytes(&bytes, b"}app.dark-forge.dev {"));
    let rejected = Command::new("caddy")
        .args(["validate", "--config"])
        .arg(&live)
        .args(["--adapter", "caddyfile"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    append_site(&live, &site);
    let repaired = fs::read(&live).unwrap();
    append_site(&live, &site);
    assert_eq!(fs::read(&live).unwrap(), repaired);
    let text = String::from_utf8(repaired.clone()).unwrap();
    assert_eq!(text.matches("app.dark-forge.dev {").count(), 1);
    assert!(!text.contains("}app.dark-forge.dev"));
    assert!(!repaired.contains(&b'\r'));
    let checked = Command::new("caddy")
        .args(["validate", "--config"])
        .arg(&live)
        .args(["--adapter", "caddyfile"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

fn test_append_keeps_crlf_comments_and_indentation() {
    let site_path = root().join("deploy/app.Caddyfile");
    let site = fs::read(&site_path).unwrap();
    let header_end = site.iter().position(|byte| *byte == b'\n').unwrap();
    let header = &site[..header_end];
    assert_eq!(header, b"app.dark-forge.dev {");
    let body = &site[header_end + 1..];
    let body_crlf = replace_bytes(body, b"\n", b"\r\n");
    let prefix = b"example.test {\r\n\trespond \"ok\"\r\n";
    let mut glued = prefix.to_vec();
    glued.push(b'}');
    glued.extend_from_slice(header);
    glued.extend_from_slice(b"\r\n");
    glued.extend_from_slice(&body_crlf);
    let mut already = prefix.to_vec();
    already.extend_from_slice(b"}\r\n");
    already.extend_from_slice(header);
    already.extend_from_slice(b"\r\n");
    already.extend_from_slice(&body_crlf);
    let comment =
        b"example.test {\n\trespond \"ok\"\n}\n# keep }app.dark-forge.dev { in this note\n";
    let indented = b"example.test {\n\trespond \"ok\"\n  }app.dark-forge.dev {\n}\n";
    let tmp = tempfile::tempdir().unwrap();
    let glued_path = tmp.path().join("glued");
    fs::write(&glued_path, &glued).unwrap();
    append_site(&glued_path, &site_path);
    let repaired = fs::read(&glued_path).unwrap();
    append_site(&glued_path, &site_path);
    assert_eq!(fs::read(&glued_path).unwrap(), repaired);
    assert!(!replace_bytes(&repaired, b"\r\n", b"").contains(&b'\n'));
    assert!(repaired.starts_with(prefix));
    assert!(repaired.ends_with(&body_crlf));
    assert_eq!(
        repaired
            .windows(20)
            .filter(|window| *window == b"app.dark-forge.dev {")
            .count(),
        1
    );
    assert!(!has_bytes(&repaired, b"}app.dark-forge.dev"));
    let checked = Command::new("caddy")
        .args(["validate", "--config"])
        .arg(&glued_path)
        .args(["--adapter", "caddyfile"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(!String::from_utf8_lossy(&checked.stderr).contains("ambiguous"));
    let split_path = tmp.path().join("split");
    fs::write(&split_path, &already).unwrap();
    append_site(&split_path, &site_path);
    assert_eq!(fs::read(&split_path).unwrap(), already);
    let comment_path = tmp.path().join("comment");
    fs::write(&comment_path, comment).unwrap();
    append_site(&comment_path, &site_path);
    let commented = fs::read(&comment_path).unwrap();
    assert!(commented.starts_with(comment));
    assert!(
        commented
            .windows(42)
            .any(|window| window == b"# keep }app.dark-forge.dev { in this note\n")
    );
    assert_eq!(
        commented
            .windows(21)
            .filter(|window| *window == b"\napp.dark-forge.dev {")
            .count(),
        1
    );
    let indent_path = tmp.path().join("indent");
    fs::write(&indent_path, indented).unwrap();
    append_site(&indent_path, &site_path);
    let indented_out = fs::read(&indent_path).unwrap();
    assert!(indented_out.starts_with(indented));
    assert!(has_bytes(&indented_out, b"\n  }app.dark-forge.dev {\n"));
    let plain = tmp.path().join("plain");
    fs::write(&plain, b"example.test {\r\n\trespond \"ok\"\r\n}\r\n").unwrap();
    append_site(&plain, &site_path);
    let appended = fs::read(&plain).unwrap();
    append_site(&plain, &site_path);
    assert_eq!(fs::read(&plain).unwrap(), appended);
    assert!(appended.starts_with(b"example.test {\r\n\trespond \"ok\"\r\n}\r\n"));
    assert!(!replace_bytes(&appended, b"\r\n", b"").contains(&b'\n'));
    assert_eq!(
        appended
            .windows(20)
            .filter(|window| *window == b"app.dark-forge.dev {")
            .count(),
        1
    );
    let plain_checked = Command::new("caddy")
        .args(["validate", "--config"])
        .arg(&plain)
        .args(["--adapter", "caddyfile"])
        .output()
        .unwrap();
    assert!(
        plain_checked.status.success(),
        "{}",
        String::from_utf8_lossy(&plain_checked.stderr)
    );
}

fn test_ci_installs_caddy_from_one_checked_archive() {
    let workflow = read(&root().join(".github/workflows/build.yml"));
    let script_path = root().join("scripts/install-caddy.sh");
    assert!(script_path.is_file());
    let script = read(&script_path);
    assert_eq!(workflow.matches("bash scripts/install-caddy.sh").count(), 2);
    assert!(!workflow.contains("caddy_2.10.2_linux_amd64.tar.gz"));
    assert_eq!(script.matches("caddy_2.10.2_linux_amd64.tar.gz").count(), 1);
    assert!(script.contains("sha512sum -c -"));
    let tmp = tempfile::tempdir().unwrap();
    let bindir = tmp.path().join("bin");
    fs::create_dir(&bindir).unwrap();
    let log = tmp.path().join("sudo.log");
    executable(
        &bindir.join("curl"),
        "#!/bin/bash\nprev=\nout=\nfor arg in \"$@\"; do\n  if [ \"$prev\" = -o ]; then out=$arg; fi\n  prev=$arg\ndone\nprintf '%s' 'not a tarball' > \"$out\"\n",
    );
    executable(
        &bindir.join("sudo"),
        &format!("#!/bin/bash\nprintf '%s\\n' \"$*\" >> {}\n", log.display()),
    );
    let mut path = std::env::var("PATH").unwrap_or_default();
    path = format!("{}:{path}", bindir.display());
    let completed = Command::new("bash")
        .arg(&script_path)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        !completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stdout)
    );
    assert!(String::from_utf8_lossy(&completed.stdout).contains("FAILED"));
    assert!(String::from_utf8_lossy(&completed.stderr).contains("computed checksum did NOT match"));
    assert!(!log.exists());
}

fn rows(lines: &[&str]) -> String {
    lines.join("\n")
}

fn test_dns_compares_every_address() {
    let same_app = rows(&[
        "10.0.0.2 STREAM app.dark-forge.dev",
        "10.0.0.2 DGRAM",
        "10.0.0.2 RAW",
        "10.0.0.1 STREAM app.dark-forge.dev",
        "10.0.0.1 DGRAM",
        "10.0.0.1 RAW",
        "",
    ]);
    let same_apex = rows(&[
        "10.0.0.1 STREAM dark-forge.dev",
        "10.0.0.1 DGRAM",
        "10.0.0.1 RAW",
        "10.0.0.2 STREAM dark-forge.dev",
        "10.0.0.2 DGRAM",
        "10.0.0.2 RAW",
        "",
    ]);
    let extra_app = rows(&[
        "10.0.0.1 STREAM app.dark-forge.dev",
        "10.0.0.1 DGRAM",
        "10.0.0.1 RAW",
        "10.0.0.2 STREAM app.dark-forge.dev",
        "10.0.0.2 DGRAM",
        "10.0.0.2 RAW",
        "",
    ]);
    let one_apex = rows(&[
        "10.0.0.1 STREAM dark-forge.dev",
        "10.0.0.1 DGRAM",
        "10.0.0.1 RAW",
        "",
    ]);
    let cases = [
        (
            "same set in a different order",
            same_app.clone(),
            same_apex,
            false,
        ),
        (
            "first row matches and a later address does not",
            extra_app,
            one_apex.clone(),
            true,
        ),
        (
            "app.dark-forge.dev has no address",
            String::new(),
            one_apex,
            true,
        ),
        (
            "dark-forge.dev has no address",
            same_app,
            String::new(),
            true,
        ),
    ];
    for (name, app_rows, apex_rows, restore) in cases {
        let (restored, reloaded) = dns_check(&app_rows, &apex_rows, 0);
        assert_eq!(restored, restore, "{name}");
        assert_eq!(reloaded, !restore, "{name}");
    }
}

fn test_dns_reload_follows_validate_exit() {
    let rows = "78.46.98.73 STREAM name\n78.46.98.73 DGRAM\n78.46.98.73 RAW\n";
    let app = rows.replace("name", "app.dark-forge.dev");
    let apex = rows.replace("name", "dark-forge.dev");
    let (restored, reloaded) = dns_check(&app, &apex, 1);
    assert!(restored);
    assert!(!reloaded);
    let (kept, reloaded_ok) = dns_check(&app, &apex, 0);
    assert!(!kept);
    assert!(reloaded_ok);
}

fn test_rollback_reload_follows_validate() {
    let real = which("caddy");
    let tmp = tempfile::tempdir().unwrap();
    let (admin_port, http_port) = two_ports();
    let admin = format!("127.0.0.1:{admin_port}");
    let live = tmp.path().join("Caddyfile");
    let backup = tmp.path().join("Caddyfile.bak");
    fs::write(
        &live,
        format!(
            "{{\n\tadmin {admin}\n\tauto_https off\n}}\nhttp://127.0.0.1:{http_port} {{\n\tbind 127.0.0.1\n\trespond \"live\"\n}}\n"
        ),
    )
    .unwrap();
    let home = tmp.path().join("home");
    let data = tmp.path().join("xdg");
    let config = tmp.path().join("xdg-config");
    for path in [&home, &data, &config] {
        fs::create_dir(path).unwrap();
    }
    let server_log = File::create(tmp.path().join("caddy-server.log")).unwrap();
    let started = Command::new(&real)
        .args(["start", "--config"])
        .arg(&live)
        .args(["--adapter", "caddyfile"])
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CONFIG_HOME", &config)
        .stdout(Stdio::from(server_log.try_clone().unwrap()))
        .stderr(Stdio::from(server_log))
        .status()
        .unwrap();
    assert!(started.success());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_http(&format!("http://127.0.0.1:{http_port}/"), "live");
        fs::write(
            &backup,
            format!("http://127.0.0.1:{http_port} {{\n\tnot_a_directive\n}}\n"),
        )
        .unwrap();
        let (failed_out, failed_log) = rollback(
            tmp.path(),
            &live,
            &backup,
            &admin,
            &real,
            &home,
            &data,
            &config,
        );
        assert!(failed_out.contains("Restored Caddyfile failed validation. Did not reload."));
        assert!(!failed_log.contains("reload"));
        assert_eq!(curl_body(&format!("http://127.0.0.1:{http_port}/")), "live");
        fs::write(
            &backup,
            format!(
                "{{\n\tadmin {admin}\n\tauto_https off\n}}\nhttp://127.0.0.1:{http_port} {{\n\tbind 127.0.0.1\n\trespond \"restored\"\n}}\n"
            ),
        )
        .unwrap();
        let (reloaded_out, reloaded_log) = rollback(
            tmp.path(),
            &live,
            &backup,
            &admin,
            &real,
            &home,
            &data,
            &config,
        );
        assert!(!reloaded_out.contains("Did not reload."));
        assert!(reloaded_log.contains("reload"));
        assert_eq!(
            curl_body(&format!("http://127.0.0.1:{http_port}/")),
            "restored"
        );
    }));
    let _ = Command::new(&real)
        .args(["stop", "--address", &admin])
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CONFIG_HOME", &config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

fn which(name: &str) -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return candidate.display().to_string();
        }
    }
    panic!("{name} is not installed");
}

pub fn cli(_args: &[String]) -> i32 {
    crate::run_cases(&cases())
}

register_cases! {
    test_pinned_address_matches_the_door_socket,
    test_ca_bind_is_read_only_at_create_and_is_not_the_key_directory,
    test_door_site_trusts_the_mounted_ca_with_the_current_directive,
    test_preflight_aborts_when_no_subnet_contains_the_pin,
    test_recreate_keeps_the_address_the_ca_and_the_401,
    test_site_is_cleartext_and_the_runbook_can_restore_identity,
    test_append_repairs_a_missing_newline_and_does_not_repeat,
    test_append_repairs_a_glued_site,
    test_append_keeps_crlf_comments_and_indentation,
    test_ci_installs_caddy_from_one_checked_archive,
    test_dns_compares_every_address,
    test_dns_reload_follows_validate_exit,
    test_rollback_reload_follows_validate,
}
