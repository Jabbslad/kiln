use crate::register_cases;
use crate::repo_root;
use flate2::Compression;
use flate2::write::GzEncoder;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SERVER_FILES: &[&str] = &[
    "bin/kiln-runtime",
    "bin/kiln",
    "bin/kiln-host",
    "bin/kiln-api",
    "install.sh",
    "fetch-firecracker.sh",
    "README.md",
    "docs/releases.md",
    "docs/remote-client.md",
    "docs/runtime.md",
    "docs/identity-service.md",
    "docs/runbooks/2026-10-19-public-door.md",
    "deploy/kiln-host.service",
    "deploy/kiln-api.service",
    "deploy/kiln-door.service",
    "deploy/enroll-dark-forge.sh",
    "deploy/host.example.json",
    "image/image.json",
    "image/inputs.json",
    "image/vmlinux",
    "image/rootfs.ext4",
    "release.json",
];

const CADDY_PAIR: &[&str] = &["deploy/paseo-caddy.override.yaml", "deploy/door.Caddyfile"];

const SERVER_INSTALLER: &str = r#"#!/bin/bash
set -euo pipefail
expected="--address ${EXPECTED_ADDRESS} --apply"
if [ -n "${EXPECTED_UPLINK}" ]; then expected="${expected} --network-uplink ${EXPECTED_UPLINK}"; fi
[ "$*" = "$expected" ]
printf '%s\n' 'provisioner completed'
"#;

const SHELL_ARCHIVE_INSTALLER: &str = r#"#!/bin/bash
set -euo pipefail
got="$*"
expected="--address ${EXPECTED_ADDRESS} --apply"
if [ -n "${EXPECTED_UPLINK}" ]; then
  expected="${expected} --network-uplink ${EXPECTED_UPLINK}"
fi
[ "$got" = "$expected" ]
printf '%s\n' 'provisioner completed'
"#;

const APPLY_UPDATED: &str = r#"#!/bin/bash
set -euo pipefail
[ "$*" = --apply ]
printf '%s\n' updated
"#;

const PROVISIONER_COMPLETED: &str = r#"#!/bin/bash
set -euo pipefail
printf '%s\n' 'provisioner completed'
"#;

const UPDATED_BARE: &str = r#"#!/bin/bash
printf '%s\n' updated
"#;

const CURL_STUB: &str = r##"#!/bin/bash
set -euo pipefail
json_quote() {
  local s=$1
  local i=0
  local n=${#s}
  local c out=""
  while [ "$i" -lt "$n" ]; do
    c=${s:i:1}
    case "$c" in
      \\) out+="\\\\" ;;
      '"') out+="\\\"" ;;
      $'\n') out+='\n' ;;
      $'\r') out+='\r' ;;
      $'\t') out+='\t' ;;
      *) out+="$c" ;;
    esac
    i=$((i + 1))
  done
  printf '"%s"' "$out"
}
log_request() {
  local dest=$1
  shift
  {
    printf '{"args": ['
    local i=0 arg
    for arg in "$@"; do
      if [ "$i" -gt 0 ]; then printf ', '; fi
      json_quote "$arg"
      i=$((i + 1))
    done
    printf ']}\n'
  } >> "$dest"
}
args=("$@")
if [ "${args[0]:-}" != "-q" ]; then
  printf '%s\n' 'curl user configuration must be disabled' >&2
  exit 1
fi
for arg in "${args[@]}"; do
  case "$arg" in
    --location-trusted|--netrc|--config|--user)
      printf '%s\n' "forbidden curl option $arg" >&2
      exit 1
      ;;
  esac
  case "$arg" in
    *Authorization*)
      printf '%s\n' 'curl must not send Authorization' >&2
      exit 1
      ;;
  esac
done
has_location=0
proto=""
proto_redir=""
output=""
write_out=0
prev=""
for arg in "${args[@]}"; do
  if [ "$arg" = "--location" ]; then has_location=1; fi
  if [ "$prev" = "--proto" ]; then proto=$arg; fi
  if [ "$prev" = "--proto-redir" ]; then proto_redir=$arg; fi
  if [ "$prev" = "--output" ]; then output=$arg; fi
  if [ "$arg" = "--write-out" ]; then write_out=1; fi
  prev=$arg
done
if [ "$has_location" -ne 1 ]; then
  printf '%s\n' 'missing --location' >&2
  exit 1
fi
if [ "$proto" != "=https" ]; then
  printf '%s\n' 'bad --proto' >&2
  exit 1
fi
if [ "$proto_redir" != "=https" ]; then
  printf '%s\n' 'bad --proto-redir' >&2
  exit 1
fi
url="${args[${#args[@]}-1]}"
prefix="https://github.com/Jabbslad/kiln/releases/download/v${PINNED}/kiln-"
case "$url" in
  "$prefix"*) ;;
  *)
    printf '%s\n' "unexpected download url $url" >&2
    exit 1
    ;;
esac
log_request "$FIXTURE/requests" "${args[@]}"
if [ "$HTTP" = "200" ]; then
  cp "$FIXTURE/archive.tar.gz" "$output"
else
  printf '%s' 'not an archive' > "$output"
fi
if [ "$write_out" -eq 1 ]; then
  printf '%s' "$HTTP"
fi
"##;

const SUDO_STUB: &str = r##"#!/bin/bash
set -euo pipefail
json_quote() {
  local s=$1
  local i=0
  local n=${#s}
  local c out=""
  while [ "$i" -lt "$n" ]; do
    c=${s:i:1}
    case "$c" in
      \\) out+="\\\\" ;;
      '"') out+="\\\"" ;;
      $'\n') out+='\n' ;;
      $'\r') out+='\r' ;;
      $'\t') out+='\t' ;;
      *) out+="$c" ;;
    esac
    i=$((i + 1))
  done
  printf '"%s"' "$out"
}
log_args() {
  local dest=$1
  shift
  {
    printf '['
    local i=0 arg
    for arg in "$@"; do
      if [ "$i" -gt 0 ]; then printf ', '; fi
      json_quote "$arg"
      i=$((i + 1))
    done
    printf ']\n'
  } >> "$dest"
}
if [ "${1:-}" != "-n" ]; then
  printf '%s\n' 'sudo must never prompt' >&2
  exit 1
fi
shift
args=("$@")
stdin=$(cat; printf x)
stdin=${stdin%x}
if [ "${args[0]:-}" = "sh" ] && [ "${args[1]:-}" = "-s" ]; then
  args+=("$stdin")
else
  if [ -n "$stdin" ]; then
    printf '%s\n' 'privileged commands must not consume script stdin' >&2
    exit 1
  fi
fi
log_args "$FIXTURE/privileged" "${args[@]}"
if [ "${args[0]:-}" = "true" ]; then
  exit "${SUDO_FAILURE:-0}"
fi
if [ "${args[0]:-}" = "bash" ]; then
  exec "${args[@]}"
fi
if [ "${args[0]:-}" = "python3" ]; then
  for arg in "${args[@]:1}"; do
    case "$arg" in
      *bind*)
        case "$arg" in
          *socket*) exit "${BIND_FAIL:-0}" ;;
        esac
        ;;
    esac
  done
  exit 0
fi
if [ "${args[0]:-}" = "sh" ] && [ "${#args[@]}" -ge 3 ] && [[ "${args[2]}" == *kvm* ]]; then
  if [ -e "$FIXTURE/kvm" ]; then
    exit 0
  fi
  exit 1
fi
if [ "${args[0]:-}" = "env" ]; then
  for i in "${!args[@]}"; do
    if [ "${args[$i]}" = "apt-get" ]; then
      sub="${args[$((i + 1))]:-}"
      case "$sub" in
        upgrade|dist-upgrade|full-upgrade)
          printf '%s\n' "refusing apt subcommand: ${args[*]}" >&2
          exit 1
          ;;
      esac
      if [ "${APT_FAIL:-}" = "$sub" ]; then
        exit 100
      fi
      exit 0
    fi
  done
fi
exit 0
"##;

const PYTHON3_STUB: &str = r#"#!/bin/bash
for arg in "$@"; do
  case "$arg" in
    *bind*)
      case "$arg" in
        *socket*) exit "${BIND_FAIL:-0}" ;;
      esac
      ;;
  esac
done
exit 0
"#;

const UNAME_STUB: &str = r#"#!/bin/sh
case "$1" in -s) echo "$OS";; -m) echo "$ARCH";; esac
"#;

const GETCONF_STUB: &str = r#"#!/bin/sh
echo "$GLIBC"
"#;

const ID_STUB: &str = "#!/bin/sh\necho 1000\n";

const IP_STUB: &str = r#"#!/bin/sh
[ "$*" = "-4 route get 1.1.1.1" ] || exit 2
printf "%s\n" "$ROUTE"
"#;

const DF_STUB: &str = r#"#!/bin/sh
[ "$1" = -B1 ] || exit 2
printf '%s\n' 'Filesystem 1B-blocks Used Available Use% Mounted' 'fixture 30000000000 1 30000000000 1% /'
"#;

const DF_LOW_STUB: &str = r#"#!/bin/sh
printf '%s\n' 'Filesystem 1B-blocks Used Available Use% Mounted' 'tmp 1 1 1 100% /'
"#;

#[derive(Clone)]
enum Member {
    File(Vec<u8>),
    Link(String),
}

#[derive(Clone)]
struct Archive {
    entries: Vec<(String, Member)>,
}

impl Archive {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn insert(&mut self, name: &str, member: Member) {
        if let Some((_, slot)) = self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == name)
        {
            *slot = member;
        } else {
            self.entries.push((name.to_string(), member));
        }
    }

    fn remove(&mut self, name: &str) {
        self.entries.retain(|(existing, _)| existing != name);
    }

    fn files(names: &[&str], bytes: &[u8]) -> Self {
        let mut archive = Self::new();
        for name in names {
            archive.insert(name, Member::File(bytes.to_vec()));
        }
        archive
    }
}

fn file_text(text: &str) -> Member {
    Member::File(text.as_bytes().to_vec())
}

fn fixture_bytes() -> Member {
    Member::File(b"fixture\n".to_vec())
}

fn server_fixture_files() -> Archive {
    let mut archive = Archive::files(SERVER_FILES, b"fixture\n");
    archive.insert("bin/kiln-network", fixture_bytes());
    archive
}

fn server_with(extra: &[&str]) -> Archive {
    let mut archive = server_fixture_files();
    for name in extra {
        archive.insert(name, fixture_bytes());
    }
    archive
}

fn server_package() -> Archive {
    let mut archive = server_fixture_files();
    archive.insert("install.sh", file_text(SERVER_INSTALLER));
    archive
}

fn client_script(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'kiln {version}'\n").into_bytes()
}

fn install_text() -> String {
    fs::read_to_string(repo_root().join("deploy/bootstrap/install.sh")).expect("read install.sh")
}

fn pinned_version() -> String {
    let text = install_text();
    let re = Regex::new(r"(?m)^    version=([0-9]+\.[0-9]+\.[0-9]+)$").expect("version regex");
    re.captures(&text)
        .unwrap_or_else(|| panic!("install.sh does not pin a release version"))
        .get(1)
        .expect("version group")
        .as_str()
        .to_string()
}

fn pinned_digests() -> Vec<(String, String)> {
    let text = install_text();
    let re = Regex::new(r"(?m)^(client|server):(\S+) ([0-9a-f]{64})$").expect("digest regex");
    let version = pinned_version();
    let mut digests = Vec::new();
    let mut seen = HashMap::<String, String>::new();
    for caps in re.captures_iter(&text) {
        let mode = caps.get(1).expect("mode").as_str();
        let target = caps.get(2).expect("target").as_str();
        let digest = caps.get(3).expect("digest").as_str();
        let name = match mode {
            "client" => format!("kiln-v{version}-{target}.tar.gz"),
            "server" => format!("kiln-server-v{version}-{target}.tar.gz"),
            _ => unreachable!(),
        };
        if seen.contains_key(&name) {
            panic!("install.sh pins {name} twice");
        }
        seen.insert(name.clone(), digest.to_string());
        digests.push((name, digest.to_string()));
    }
    if digests.is_empty() {
        panic!("install.sh does not pin release archives");
    }
    digests
}

fn sha256_file(path: &Path) -> String {
    let mut file =
        File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).expect("hash archive");
    format!("{:x}", hasher.finalize())
}

fn download_url(url: &str, dest: &Path) {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .expect("http client");
    let mut response = client.get(url).send().unwrap_or_else(|error| {
        panic!("cannot download {url}: {error}");
    });
    if !response.status().is_success() {
        panic!("cannot download {url}: HTTP {}", response.status());
    }
    let mut file =
        File::create(dest).unwrap_or_else(|error| panic!("create {}: {error}", dest.display()));
    io::copy(&mut response, &mut file).unwrap_or_else(|error| panic!("save {url}: {error}"));
}

fn published_server_names() -> Vec<String> {
    let text = install_text();
    let re =
        Regex::new(r"(?m)^server:x86_64-unknown-linux-gnu ([0-9a-f]{64})$").expect("server pin");
    let digest = re
        .captures(&text)
        .unwrap_or_else(|| panic!("install.sh does not pin the server archive"))
        .get(1)
        .expect("server digest")
        .as_str()
        .to_string();
    let version = pinned_version();
    let name = format!("kiln-server-v{version}-x86_64-unknown-linux-gnu.tar.gz");
    let cache = PathBuf::from("/tmp").join(&name);
    if !cache.is_file() || sha256_file(&cache) != digest {
        download_url(
            &format!("https://github.com/Jabbslad/kiln/releases/download/v{version}/{name}"),
            &cache,
        );
    }
    let actual = sha256_file(&cache);
    assert!(
        actual == digest,
        "published server archive checksum is {actual}"
    );
    let output = Command::new("tar")
        .args(["-tzf"])
        .arg(&cache)
        .output()
        .expect("tar list");
    assert!(
        output.status.success(),
        "tar list: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect()
}

fn set_archive_path(header: &mut tar::Header, name: &str) {
    if header.set_path(name).is_ok() {
        return;
    }
    let slot = &mut header.as_old_mut().name;
    slot.fill(0);
    let bytes = name.as_bytes();
    assert!(bytes.len() < slot.len(), "archive name too long: {name}");
    slot[..bytes.len()].copy_from_slice(bytes);
}

fn prepare_header(name: &str, size: u64, mode: u32, kind: tar::EntryType) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(size);
    header.set_username("").expect("tar username");
    header.set_groupname("").expect("tar groupname");
    header.set_entry_type(kind);
    set_archive_path(&mut header, name);
    header
}

fn write_archive(path: &Path, archive: &Archive) {
    let file =
        File::create(path).unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, member) in &archive.entries {
        match member {
            Member::File(data) => {
                let mut header =
                    prepare_header(name, data.len() as u64, 0o755, tar::EntryType::Regular);
                header.set_cksum();
                builder
                    .append(&header, data.as_slice())
                    .unwrap_or_else(|error| panic!("tar append {name}: {error}"));
            }
            Member::Link(target) => {
                let mut header = prepare_header(name, 0, 0o644, tar::EntryType::Symlink);
                header
                    .set_link_name(target)
                    .unwrap_or_else(|error| panic!("tar link {target}: {error}"));
                header.set_cksum();
                builder
                    .append(&header, io::empty())
                    .unwrap_or_else(|error| panic!("tar append link {name}: {error}"));
            }
        }
    }
    let encoder = builder.into_inner().expect("finish tar");
    let file = encoder.finish().expect("finish gzip");
    file.sync_all().expect("sync archive");
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

fn contains_ascii_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn ends_with(command: &[String], suffix: &[&str]) -> bool {
    command.len() >= suffix.len()
        && command[command.len() - suffix.len()..]
            .iter()
            .zip(suffix)
            .all(|(left, right)| left == right)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn apt_subcommands(commands: &[Vec<String>]) -> Vec<String> {
    let mut found = Vec::new();
    for command in commands {
        if let Some(index) = command.iter().position(|part| part == "apt-get") {
            found.push(command[index + 1].clone());
        }
    }
    found
}

fn sqlite_update() -> Vec<String> {
    strings(&[
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "NEEDRESTART_MODE=l",
        "apt-get",
        "update",
    ])
}

fn sqlite_install() -> Vec<String> {
    strings(&[
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "NEEDRESTART_MODE=l",
        "apt-get",
        "install",
        "-y",
        "--no-install-recommends",
        "--no-upgrade",
        "sqlite3",
    ])
}

fn request_url(request: &Value) -> &str {
    request["args"]
        .as_array()
        .unwrap_or_else(|| panic!("request missing args: {request}"))
        .last()
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("request missing url: {request}"))
}

fn version_numbers(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or_else(|_| panic!("version {version}")))
        .collect()
}

struct Fixture {
    root: PathBuf,
    archive: PathBuf,
    env: HashMap<String, String>,
    _temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        assert!(
            repo_root().join("deploy/bootstrap/install.sh").is_file(),
            "bootstrap is missing"
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        for name in ["home", "bin", "tmp", "systemd"] {
            fs::create_dir(root.join(name)).expect("fixture dir");
        }
        fs::write(root.join("os-release"), "ID=ubuntu\nVERSION_ID=\"24.04\"\n")
            .expect("os-release");
        fs::write(root.join("kvm"), b"").expect("kvm");
        fs::write(root.join("controllers"), "cpu memory pids\n").expect("controllers");
        fs::write(root.join("meminfo"), "MemAvailable:   8388608 kB\n").expect("meminfo");
        let mut env: HashMap<String, String> = std::env::vars().collect();
        let path = std::env::var("PATH").expect("PATH");
        env.insert("HOME".into(), root.join("home").display().to_string());
        env.insert("TMPDIR".into(), root.join("tmp").display().to_string());
        env.insert(
            "PATH".into(),
            format!("{}:{path}", root.join("bin").display()),
        );
        env.insert("FIXTURE".into(), root.display().to_string());
        env.insert("OS".into(), "Linux".into());
        env.insert("ARCH".into(), "x86_64".into());
        env.insert("GLIBC".into(), "glibc 2.39".into());
        env.insert("HTTP".into(), "200".into());
        env.insert(
            "ROUTE".into(),
            "1.1.1.1 via 192.168.50.1 dev eth0 src 192.168.50.7 uid 1000\n    cache".into(),
        );
        env.insert("EXPECTED_ADDRESS".into(), "192.168.50.7".into());
        env.insert("EXPECTED_UPLINK".into(), String::new());
        env.insert("PINNED".into(), pinned_version());
        env.remove("KILN_NETWORK_UPLINK");
        let fixture = Self {
            archive: root.join("archive.tar.gz"),
            root,
            env,
            _temp: temp,
        };
        fixture.executable("uname", UNAME_STUB);
        fixture.executable("getconf", GETCONF_STUB);
        fixture.executable("id", ID_STUB);
        fixture.executable("ip", IP_STUB);
        fixture.executable("df", DF_STUB);
        fixture.executable("curl", CURL_STUB);
        fixture.executable("python3", PYTHON3_STUB);
        fixture.executable("sudo", SUDO_STUB);
        fixture
    }

    fn executable(&self, name: &str, text: &str) {
        let path = self.root.join("bin").join(name);
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {name}: {error}"));
        let mut permissions = fs::metadata(&path).expect("stub metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("stub mode");
    }

    fn package(&self, archive: &Archive) -> String {
        write_archive(&self.archive, archive);
        sha256_file(&self.archive)
    }

    fn package_client(&self) -> String {
        let mut archive = Archive::new();
        archive.insert("kiln", Member::File(client_script(&pinned_version())));
        self.package(&archive)
    }

    fn package_server(&self) -> String {
        self.package(&server_package())
    }

    fn write_script(&self, digest: &str) -> PathBuf {
        let source = install_text();
        let (start, tail) = source
            .split_once("# BEGIN RELEASE ASSETS\n")
            .expect("begin release assets");
        let (_, end) = tail
            .split_once("# END RELEASE ASSETS")
            .expect("end release assets");
        let rows = [
            "client:x86_64-unknown-linux-gnu",
            "client:x86_64-apple-darwin",
            "client:aarch64-apple-darwin",
            "server:x86_64-unknown-linux-gnu",
        ]
        .into_iter()
        .map(|key| format!("{key} {digest}"))
        .collect::<Vec<_>>()
        .join("\n");
        let mut source =
            format!("{start}# BEGIN RELEASE ASSETS\n{rows}\n# END RELEASE ASSETS{end}");
        for (old, new) in [
            ("/etc/os-release", "os-release"),
            ("/dev/kvm", "kvm"),
            ("/proc/meminfo", "meminfo"),
            ("/run/systemd/system", "systemd"),
            ("/sys/fs/cgroup/cgroup.controllers", "controllers"),
            ("/etc/kiln", "etc-kiln"),
            ("/opt/kiln", "opt-kiln"),
            ("/var/lib/kiln", "state-kiln"),
        ] {
            source = source.replace(old, &self.root.join(new).display().to_string());
        }
        let path = self.root.join("install.sh");
        fs::write(&path, source).expect("write rewritten install.sh");
        path
    }

    fn run(&self, digest: &str, args: &[&str]) -> (i32, String) {
        let script = self.write_script(digest);
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("file=$1; shift; cat \"$file\" | sh -s -- \"$@\"")
            .arg("test")
            .arg(&script)
            .args(args)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn().expect("spawn bootstrap");
        let (code, stdout, stderr) = capture(child);
        let output = format!("{stdout}{stderr}");
        assert!(!output.contains("GitHub token:"), "{output}");
        let leftovers: Vec<_> = fs::read_dir(self.root.join("tmp"))
            .expect("tmp")
            .map(|entry| entry.expect("tmp entry").file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remain: {leftovers:?}"
        );
        (code, output)
    }

    fn requests(&self) -> Vec<Value> {
        let path = self.root.join("requests");
        if !path.exists() {
            return Vec::new();
        }
        fs::read_to_string(&path)
            .expect("requests")
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("request json: {error}: {line}"))
            })
            .collect()
    }

    fn privileged_text(&self) -> String {
        let path = self.root.join("privileged");
        if !path.exists() {
            return String::new();
        }
        fs::read_to_string(path).expect("privileged")
    }

    fn privileged_commands(&self) -> Vec<Vec<String>> {
        let text = self.privileged_text();
        if text.is_empty() {
            return Vec::new();
        }
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let value: Value = serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("privileged json: {error}: {line}"));
                value
                    .as_array()
                    .unwrap_or_else(|| panic!("privileged row is not a list: {line}"))
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .unwrap_or_else(|| panic!("privileged arg is not a string: {line}"))
                            .to_string()
                    })
                    .collect()
            })
            .collect()
    }

    fn privileged_lines(&self) -> Vec<String> {
        let path = self.root.join("privileged");
        if !path.exists() {
            return Vec::new();
        }
        self.privileged_text()
            .lines()
            .map(|line| line.to_string())
            .collect()
    }

    fn old_client(&self) -> PathBuf {
        let binary = self.client_binary();
        fs::create_dir_all(binary.parent().expect("client dir")).expect("client dir");
        fs::write(&binary, "#!/bin/sh\necho 'kiln 0.1.1'\n").expect("old client");
        let mut permissions = fs::metadata(&binary)
            .expect("old client metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).expect("old client mode");
        binary
    }

    fn client_binary(&self) -> PathBuf {
        self.root.join("home/.local/bin/kiln")
    }

    fn path_without(&self, name: &str) -> String {
        let hidden = self.root.join(format!("hide-{name}"));
        fs::create_dir(&hidden).unwrap_or_else(|error| panic!("hide {name}: {error}"));
        for directory in std::env::var("PATH").expect("PATH").split(':') {
            let base = Path::new(directory);
            if !base.is_dir() {
                continue;
            }
            let Ok(entries) = fs::read_dir(base) else {
                continue;
            };
            for entry in entries.flatten() {
                let entry_name = entry.file_name();
                let entry_name = entry_name.to_string_lossy();
                if entry_name == name {
                    continue;
                }
                let dest = hidden.join(entry_name.as_ref());
                if dest.exists() {
                    continue;
                }
                let _ = symlink(entry.path(), dest);
            }
        }
        format!("{}:{}", self.root.join("bin").display(), hidden.display())
    }
}

fn capture(mut child: Child) -> (i32, String, String) {
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > Duration::from_secs(15) => {
                let pid = child.id() as i32;
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                panic!("bootstrap timed out");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("wait bootstrap: {error}"),
        }
    };
    let stdout = String::from_utf8_lossy(&out_handle.join().expect("stdout thread")).into_owned();
    let stderr = String::from_utf8_lossy(&err_handle.join().expect("stderr thread")).into_owned();
    (status.code().unwrap_or(1), stdout, stderr)
}

fn command_stdout(program: &Path, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", program.display()));
    assert!(
        output.status.success(),
        "{} {:?}: {}",
        program.display(),
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn reject_unexpected(fix: &Fixture, archive: &Archive) {
    let digest = fix.package(archive);
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(out.contains("Unexpected archive contents."), "{out}");
}

fn accept_archive_then_fail(fix: &Fixture, archive: &Archive) {
    let digest = fix.package(archive);
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(!out.contains("Unexpected archive contents."), "{out}");
}

fn server_detects_address_and_installs_without_tty(fix: &Fixture) {
    let version = pinned_version();
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("provisioner completed"), "{out}");
    let commands = fix.privileged_commands();
    assert_eq!(commands[0], strings(&["true"]));
    assert!(commands[1][2].contains("kvm"), "{:?}", commands[1]);
    assert!(
        ends_with(&commands[2], &["apt-get", "update"]),
        "{:?}",
        commands[2]
    );
    assert!(
        !commands[3].iter().any(|arg| arg == "python3"),
        "{:?}",
        commands[3]
    );
    assert!(
        !commands[3].iter().any(|arg| arg == "sqlite3"),
        "{:?}",
        commands[3]
    );
    assert_eq!(commands[4][0], "bash");
    let requests = fix.requests();
    let url = request_url(&requests[0]);
    assert!(
        url.ends_with(&format!(
            "/kiln-server-v{version}-x86_64-unknown-linux-gnu.tar.gz"
        )),
        "{url}"
    );
}

fn test_pinned_version_matches_published_release() {
    let _fix = Fixture::new();
    let version = pinned_version();
    let url = format!("https://api.github.com/repos/Jabbslad/kiln/releases/tags/v{version}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("http client");
    let mut builder = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "kiln-bootstrap-test");
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("GH_TOKEN")
                .ok()
                .filter(|value| !value.is_empty())
        });
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let response = match builder.send() {
        Ok(response) => response,
        Err(exc) => panic!("cannot read published release v{version}: {exc}"),
    };
    if !response.status().is_success() {
        panic!(
            "cannot read published release v{version}: HTTP {}",
            response.status()
        );
    }
    let payload: Value = match response.json() {
        Ok(payload) => payload,
        Err(exc) => panic!("cannot read published release v{version}: {exc}"),
    };
    let expected_tag = format!("v{version}");
    assert_eq!(payload["tag_name"].as_str(), Some(expected_tag.as_str()));
    assert_eq!(payload["draft"].as_bool(), Some(false));
    assert_eq!(payload["prerelease"].as_bool(), Some(false));
    let mut published = HashMap::<String, Option<String>>::new();
    let assets = payload["assets"]
        .as_array()
        .unwrap_or_else(|| panic!("release assets missing"));
    for asset in assets {
        let name = asset["name"]
            .as_str()
            .unwrap_or_else(|| panic!("asset missing name: {asset}"))
            .to_string();
        let digest = match asset.get("digest") {
            Some(value) if value.is_null() => None,
            Some(value) => Some(
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("asset digest is not a string: {asset}"))
                    .to_string(),
            ),
            None => None,
        };
        published.insert(name, digest);
    }
    for (name, digest) in pinned_digests() {
        let expected = format!("sha256:{digest}");
        assert_eq!(
            published.get(&name).and_then(|value| value.as_deref()),
            Some(expected.as_str()),
            "{name}"
        );
    }
}

fn test_client_platforms_install_correct_asset() {
    let mut fix = Fixture::new();
    let version = pinned_version();
    for (system, arch, expected) in [
        ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
        ("Darwin", "x86_64", "x86_64-apple-darwin"),
        ("Darwin", "arm64", "aarch64-apple-darwin"),
    ] {
        fix.env.insert("OS".into(), system.into());
        fix.env.insert("ARCH".into(), arch.into());
        let digest = fix.package_client();
        let (code, out) = fix.run(&digest, &[]);
        assert_eq!(code, 0, "system {system} arch {arch}: {out}");
        assert!(out.contains("kiln login"), "{out}");
        assert!(
            !out.contains("Next: connect using the server enrollment bundle"),
            "{out}"
        );
        let binary = fix.client_binary();
        assert_eq!(
            command_stdout(&binary, &["--version"]),
            format!("kiln {version}\n")
        );
        let requests = fix.requests();
        let url = request_url(requests.last().expect("download recorded"));
        assert_eq!(
            url,
            format!(
                "https://github.com/Jabbslad/kiln/releases/download/v{version}/kiln-v{version}-{expected}.tar.gz"
            )
        );
        fs::remove_file(&binary).expect("unlink client");
    }
}

fn test_bad_digest_never_installs() {
    let fix = Fixture::new();
    fix.package_client();
    let (code, out) = fix.run(&"0".repeat(64), &[]);
    assert!(code != 0, "{out}");
    assert!(contains_ascii_ignore_case(&out, "checksum"), "{out}");
    assert!(!fix.client_binary().exists());
}

fn test_http_error_never_installs() {
    let mut fix = Fixture::new();
    fix.env.insert("HTTP".into(), "404".into());
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert!(out.contains("404"), "{out}");
    assert!(!fix.client_binary().exists());
}

fn test_unsafe_archives_are_rejected_even_with_correct_digest() {
    let fix = Fixture::new();
    let mut escaped = Archive::new();
    escaped.insert("../escaped", Member::File(b"bad".to_vec()));
    let mut link = Archive::new();
    link.insert("kiln", Member::Link("/bin/sh".into()));
    let mut extra = Archive::new();
    extra.insert("kiln", Member::File(b"ok".to_vec()));
    extra.insert("extra", Member::File(b"bad".to_vec()));
    for archive in [escaped, link, extra] {
        let digest = fix.package(&archive);
        let (code, out) = fix.run(&digest, &[]);
        assert!(code != 0, "{out}");
        assert!(contains_ascii_ignore_case(&out, "archive"), "{out}");
        assert!(!fix.client_binary().exists());
    }
}

fn test_existing_client_is_preserved_without_downloading() {
    let fix = Fixture::new();
    let binary = fix.client_binary();
    fs::create_dir_all(binary.parent().expect("client dir")).expect("client dir");
    fs::write(&binary, "preserve").expect("preserve client");
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert_eq!(fs::read_to_string(&binary).expect("preserved"), "preserve");
    assert!(fix.requests().is_empty());
    fs::remove_file(&binary).expect("unlink client");
    symlink("missing-target", &binary).expect("symlink client");
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert_eq!(
        fs::read_link(&binary).expect("readlink"),
        PathBuf::from("missing-target")
    );
    assert!(fix.requests().is_empty());
}

fn test_platform_and_glibc_rejections_do_not_prompt_or_download() {
    let mut fix = Fixture::new();
    for (key, value) in [
        ("OS", "FreeBSD"),
        ("ARCH", "aarch64"),
        ("GLIBC", "glibc 2.38"),
        ("GLIBC", "musl 1.2"),
    ] {
        let old = fix.env.clone();
        fix.env.insert(key.into(), value.into());
        let digest = fix.package_client();
        let (code, out) = fix.run(&digest, &[]);
        assert!(code != 0, "{key}={value}: {out}");
        assert!(!out.contains("GitHub token: "), "{out}");
        assert!(fix.requests().is_empty(), "{key}={value}");
        fix.env = old;
    }
}

fn test_rerun_replaces_client_and_keeps_previous_and_profiles() {
    let fix = Fixture::new();
    let version = pinned_version();
    let binary = fix.old_client();
    let original = fs::read(&binary).expect("original client");
    let profile = fix.root.join("home/.config/kiln/profiles.json");
    fs::create_dir_all(profile.parent().expect("profile dir")).expect("profile dir");
    fs::write(&profile, "preserve profile and credential references").expect("profile");
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        command_stdout(&binary, &["--version"]),
        format!("kiln {version}\n")
    );
    assert_eq!(
        fs::read(binary.with_file_name("kiln.previous")).expect("previous"),
        original
    );
    assert_eq!(
        fs::read_to_string(&profile).expect("profile"),
        "preserve profile and credential references"
    );
    assert_eq!(
        dir_names(binary.parent().expect("bin")),
        strings(&["kiln", "kiln.previous"])
    );
    let installed = fs::metadata(&binary).expect("installed").ino();
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("already current"), "{out}");
    assert_eq!(
        fs::metadata(&binary).expect("still installed").ino(),
        installed
    );
    assert_eq!(
        fs::read(binary.with_file_name("kiln.previous")).expect("previous"),
        original
    );
}

fn test_failed_upgrade_preserves_current_and_previous_clients() {
    let mut fix = Fixture::new();
    let binary = fix.old_client();
    let original = fs::read(&binary).expect("original client");
    let previous = binary.with_file_name("kiln.previous");
    fs::write(&previous, b"older backup").expect("older backup");
    for failure in ["checksum", "version", "HTTP"] {
        fix.env.insert(
            "HTTP".into(),
            if failure == "HTTP" { "403" } else { "200" }.into(),
        );
        let digest = if failure == "version" {
            let mut archive = Archive::new();
            archive.insert(
                "kiln",
                Member::File(b"#!/bin/sh\necho wrong-version\n".to_vec()),
            );
            fix.package(&archive)
        } else {
            fix.package_client()
        };
        let pinned = if failure == "checksum" {
            "0".repeat(64)
        } else {
            digest
        };
        let (code, out) = fix.run(&pinned, &[]);
        assert!(code != 0, "{failure}: {out}");
        assert!(
            contains_ascii_ignore_case(&out, failure),
            "{failure}: {out}"
        );
        assert_eq!(fs::read(&binary).expect("current"), original);
        assert_eq!(fs::read(&previous).expect("previous"), b"older backup");
        assert!(
            !binary
                .parent()
                .expect("bin")
                .join(".kiln-install.lock")
                .exists()
        );
    }
}

fn test_upgrade_rejects_symlinks_and_busy_lock_before_downloading() {
    let fix = Fixture::new();
    let binary = fix.old_client();
    let targets = [binary.clone(), binary.with_file_name("kiln.previous")];
    for unsafe_path in targets {
        if unsafe_path.exists() {
            fs::remove_file(&unsafe_path).expect("remove unsafe path");
        }
        symlink("missing-target", &unsafe_path).expect("unsafe symlink");
        let digest = fix.package_client();
        let (code, out) = fix.run(&digest, &[]);
        assert!(code != 0, "{out}");
        assert!(out.contains("regular file"), "{out}");
        assert!(is_symlink(&unsafe_path));
        assert!(fix.requests().is_empty());
        fs::remove_file(&unsafe_path).expect("unlink unsafe path");
        fix.old_client();
    }
    let lock = binary.parent().expect("bin").join(".kiln-install.lock");
    fs::create_dir(&lock).expect("lock");
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert!(out.contains("another installer"), "{out}");
    assert!(lock.exists());
    assert!(fix.requests().is_empty());
}

fn test_upgrade_failed_publication_preserves_working_client() {
    let fix = Fixture::new();
    let binary = fix.old_client();
    let original = fs::read(&binary).expect("original client");
    let output = Command::new("sh")
        .args(["-c", "command -v mv"])
        .output()
        .expect("locate mv");
    assert!(output.status.success(), "mv missing");
    let real_mv = String::from_utf8(output.stdout)
        .expect("mv path")
        .trim()
        .to_string();
    fix.executable(
        "mv",
        &format!(
            "#!/bin/sh\nfor arg do last=$arg; done\ncase \"$last\" in */kiln) exit 1;; esac\nexec \"{real_mv}\" \"$@\"\n"
        ),
    );
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert!(contains_ascii_ignore_case(&out, "replace"), "{out}");
    assert_eq!(fs::read(&binary).expect("current"), original);
    assert_eq!(
        fs::read(binary.with_file_name("kiln.previous")).expect("previous"),
        original
    );
    assert_eq!(
        dir_names(binary.parent().expect("bin")),
        strings(&["kiln", "kiln.previous"])
    );
}

fn test_legacy_upgrade_flag_is_an_optional_client_alias() {
    let fix = Fixture::new();
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &["client", "--upgrade"]);
    assert_eq!(code, 0, "{out}");
}

fn test_newer_client_is_not_downgraded() {
    let fix = Fixture::new();
    let binary = fix.old_client();
    let pinned = pinned_version();
    let parts = version_numbers(&pinned);
    let mut newer = vec![
        format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1),
        "0.4.10".to_string(),
        "0.5.0".to_string(),
        "1.0.0".to_string(),
    ];
    newer.sort();
    let pin = parts;
    for version in newer {
        if version_numbers(&version) <= pin {
            continue;
        }
        fs::write(&binary, format!("#!/bin/sh\necho 'kiln {version}'\n")).expect("newer client");
        let digest = fix.package_client();
        let (code, out) = fix.run(&digest, &[]);
        assert!(code != 0, "{version}: {out}");
        assert!(out.contains("newer"), "{version}: {out}");
        assert!(
            fs::read_to_string(&binary)
                .expect("client")
                .contains(&version)
        );
        assert!(!binary.with_file_name("kiln.previous").exists());
    }
}

fn test_multiline_client_version_is_refused_before_download() {
    let fix = Fixture::new();
    let binary = fix.old_client();
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'kiln 0.3.0\\nnot-a-version\\n'\n",
    )
    .expect("multiline");
    let original = fs::read(&binary).expect("original");
    let digest = fix.package_client();
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert_eq!(fs::read(&binary).expect("unchanged"), original);
    assert!(fix.requests().is_empty());
}

fn test_existing_server_skips_apt_and_route_detection() {
    let mut fix = Fixture::new();
    fs::create_dir(fix.root.join("etc-kiln")).expect("etc-kiln");
    fix.env.insert("ROUTE".into(), String::new());
    let mut members = server_fixture_files();
    members.insert("install.sh", file_text(APPLY_UPDATED));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("updated"), "{out}");
    let commands = fix.privileged_commands();
    assert_eq!(commands[0], strings(&["true"]));
    assert_eq!(commands[1][0], "sh");
    assert!(
        commands[1].join(" ").contains("disable --now"),
        "{:?}",
        commands[1]
    );
    assert_eq!(commands[2][0], "bash");
    assert_eq!(commands.len(), 3);
}

fn test_server_unsupported_host_never_requests_privileges() {
    let fix = Fixture::new();
    for (distro, version) in [
        ("ubuntu", "22.04"),
        ("ubuntu", "24.10"),
        ("ubuntu", "26.10"),
        ("debian", "12"),
        ("debian", "26.04"),
    ] {
        fs::write(
            fix.root.join("os-release"),
            format!("ID={distro}\nVERSION_ID=\"{version}\"\n"),
        )
        .expect("os-release");
        let digest = fix.package_server();
        let (code, out) = fix.run(&digest, &["server"]);
        assert!(code != 0, "{distro} {version}: {out}");
        assert!(out.contains("24.04"), "{distro} {version}: {out}");
        assert!(fix.requests().is_empty(), "{distro} {version}");
        assert!(!fix.root.join("privileged").exists(), "{distro} {version}");
    }
}

fn test_ubuntu_2604_reaches_server_provisioning() {
    let fix = Fixture::new();
    fs::write(
        fix.root.join("os-release"),
        "ID=ubuntu\nVERSION_ID=\"26.04\"\n",
    )
    .expect("os-release");
    server_detects_address_and_installs_without_tty(&fix);
}

fn test_debian_13_reaches_server_provisioning() {
    let fix = Fixture::new();
    fs::write(
        fix.root.join("os-release"),
        "ID=debian\nVERSION_ID=\"13\"\n",
    )
    .expect("os-release");
    server_detects_address_and_installs_without_tty(&fix);
}

fn test_server_detects_address_and_installs_without_tty() {
    let fix = Fixture::new();
    server_detects_address_and_installs_without_tty(&fix);
}

fn test_kvm_access_is_checked_by_privileged_provisioner() {
    let fix = Fixture::new();
    let mut permissions = fs::metadata(fix.root.join("kvm"))
        .expect("kvm")
        .permissions();
    permissions.set_mode(0o0);
    fs::set_permissions(fix.root.join("kvm"), permissions).expect("chmod kvm");
    server_detects_address_and_installs_without_tty(&fix);
}

fn test_network_auto_detection_and_explicit_overrides() {
    let mut fix = Fixture::new();
    let scenarios: &[(&[&str], &str, &str)] = &[
        (&["--network"], "192.168.50.7", "eth0"),
        (
            &["--address", "100.64.5.6", "--network-uplink", "enp2s0"],
            "100.64.5.6",
            "enp2s0",
        ),
    ];
    for &(args, address, uplink) in scenarios {
        fix.env
            .insert("EXPECTED_ADDRESS".into(), address.to_string());
        fix.env.insert("EXPECTED_UPLINK".into(), uplink.to_string());
        let mut full = vec!["server"];
        full.extend_from_slice(args);
        let digest = fix.package_server();
        let (code, out) = fix.run(&digest, &full);
        assert_eq!(code, 0, "{args:?}: {out}");
        let commands = fix.privileged_commands();
        let install = &commands[commands.len() - 2];
        for package in ["nftables", "iproute2", "util-linux"] {
            assert!(
                install.iter().any(|arg| arg == package),
                "{package} in {install:?}"
            );
        }
    }
}

fn test_invalid_addresses_do_not_run_sudo() {
    let fix = Fixture::new();
    for address in [
        "8.8.8.8",
        "100.63.0.1",
        "172.32.0.1",
        "10.0.0.999",
        "10.0.0.01",
    ] {
        let digest = fix.package_server();
        let (code, out) = fix.run(&digest, &["server", "--address", address]);
        assert!(code != 0, "{address}: {out}");
        assert!(out.contains("private/VPN"), "{address}: {out}");
        assert!(!fix.root.join("privileged").exists(), "{address}");
    }
}

fn test_bad_route_fails_without_mutations() {
    let mut fix = Fixture::new();
    for route in [
        "",
        "1.1.1.1 dev eth0 src 8.8.8.8",
        "1.1.1.1 dev eth0 src 10.1.2.3 src 10.4.5.6",
    ] {
        fix.env.insert("ROUTE".into(), route.into());
        let digest = fix.package_server();
        let (code, out) = fix.run(&digest, &["server"]);
        assert!(code != 0, "{route:?}: {out}");
        assert!(out.contains("--address"), "{route:?}: {out}");
        assert!(!fix.root.join("privileged").exists(), "{route:?}");
    }
}

fn test_explicit_address_works_without_default_route() {
    let mut fix = Fixture::new();
    fix.env.insert("ROUTE".into(), String::new());
    fix.env
        .insert("EXPECTED_ADDRESS".into(), "172.31.8.9".into());
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server", "--address", "172.31.8.9"]);
    assert_eq!(code, 0, "{out}");
}

fn test_sudo_failure_does_not_download_or_install() {
    let mut fix = Fixture::new();
    fix.env.insert("SUDO_FAILURE".into(), "1".into());
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(out.contains("sudo"), "{out}");
    assert!(fix.requests().is_empty());
    assert_eq!(fix.privileged_lines(), vec!["[\"true\"]".to_string()]);
}

fn test_bad_options_never_download_or_provision() {
    let fix = Fixture::new();
    let cases: &[&[&str]] = &[
        &["server", "--address"],
        &["server", "--network-uplink", "bad;name"],
        &["server", "--network-uplink", "eth0\nbad;name"],
        &["server", "--address", "10.1.2.3\ninvalid"],
        &["client", "--network"],
        &["server", "--unknown"],
    ];
    for args in cases {
        let digest = fix.package_server();
        let (code, out) = fix.run(&digest, args);
        assert!(code != 0, "{args:?}: {out}");
        assert!(fix.requests().is_empty(), "{args:?}");
        assert!(!fix.root.join("privileged").exists(), "{args:?}");
    }
}

fn test_network_environment_override_and_missing_helper() {
    let mut fix = Fixture::new();
    fix.env
        .insert("KILN_NETWORK_UPLINK".into(), "enp4s0".into());
    fix.env.insert("EXPECTED_UPLINK".into(), "enp4s0".into());
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    fs::remove_file(fix.root.join("privileged")).expect("clear privileged");
    let digest = fix.package(&Archive::files(SERVER_FILES, b"fixture\n"));
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(out.contains("no guest networking"), "{out}");
    assert_eq!(fix.privileged_lines(), vec!["[\"true\"]".to_string()]);
}

fn test_missing_or_ambiguous_network_device_requires_override() {
    let mut fix = Fixture::new();
    for route in [
        "1.1.1.1 src 192.168.50.7",
        "1.1.1.1 dev eth0 dev eth1 src 192.168.50.7",
    ] {
        fix.env.insert("ROUTE".into(), route.into());
        let digest = fix.package_server();
        let (code, out) = fix.run(&digest, &["server", "--network"]);
        assert!(code != 0, "{route:?}: {out}");
        assert!(out.contains("--network-uplink"), "{route:?}: {out}");
        assert!(!fix.root.join("privileged").exists(), "{route:?}");
    }
}

fn test_resource_limits_stop_install_before_apt() {
    let fix = Fixture::new();
    fs::write(fix.root.join("meminfo"), "MemAvailable:   1024 kB\n").expect("meminfo");
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(out.contains("6 GiB"), "{out}");
    assert!(!fix.privileged_text().contains("apt-get"));
}

fn test_low_disk_stops_install_before_apt() {
    let fix = Fixture::new();
    fix.executable("df", DF_LOW_STUB);
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(out.contains("24 GiB"), "{out}");
    assert!(!fix.privileged_text().contains("apt-get"));
}

fn test_bootstrap_leaves_port_binding_to_the_installer() {
    let mut fix = Fixture::new();
    let mut text = install_text();
    // The allowlist names the systemd unit. The script still does not bind.
    text = text.replace("kiln-door-proxy-public.socket", "");
    text = text.replace("kiln-door-proxy.socket", "");
    text = text.replace("kiln-app-proxy.socket", "");
    assert!(!text.contains("socket"), "{text}");
    fix.env.insert("BIND_FAIL".into(), "1".into());
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("provisioner completed"), "{out}");
}

fn test_app_units_are_required_together() {
    let fix = Fixture::new();
    let app_units = [
        "deploy/kiln-app.service",
        "deploy/kiln-app-proxy.service",
        "deploy/kiln-app-proxy.socket",
    ];
    accept_archive_then_fail(&fix, &server_with(&app_units));
    reject_unexpected(&fix, &server_with(&app_units[..1]));
}

fn test_app_site_and_runbook_ship_as_a_pair() {
    let fix = Fixture::new();
    let pair = [
        "deploy/app.Caddyfile",
        "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
    ];
    accept_archive_then_fail(&fix, &server_with(&pair));
    for name in pair {
        reject_unexpected(&fix, &server_with(&[name]));
    }
}

fn test_door_proxy_files_match_the_packager_or_are_absent_together() {
    let fix = Fixture::new();
    let door_proxy = [
        "deploy/kiln-door-proxy.service",
        "deploy/kiln-door-proxy.socket",
        "docs/runbooks/2026-10-19-door-public-cert.md",
    ];
    let public = "deploy/kiln-door-proxy-public.socket";
    let members = server_with(&door_proxy);
    accept_archive_then_fail(&fix, &members);
    let mut legacy = members;
    legacy.insert(public, fixture_bytes());
    accept_archive_then_fail(&fix, &legacy);
    reject_unexpected(&fix, &server_with(&door_proxy[..1]));
    reject_unexpected(&fix, &server_with(&[public]));
}

fn test_pinned_archive_names_pass_and_the_caddy_pair_is_independent() {
    let fix = Fixture::new();
    let names = published_server_names();
    assert!(
        names
            .iter()
            .any(|name| name == "deploy/kiln-door-proxy.service")
    );
    for name in CADDY_PAIR {
        assert!(names.iter().any(|item| item == name), "{name}");
    }
    let mut members = Archive::new();
    for name in &names {
        members.insert(name, fixture_bytes());
    }
    accept_archive_then_fail(&fix, &members);
    let mut without_door = server_fixture_files();
    for name in CADDY_PAIR {
        without_door.insert(name, fixture_bytes());
    }
    accept_archive_then_fail(&fix, &without_door);
    for name in CADDY_PAIR {
        let mut half = members.clone();
        half.remove(name);
        reject_unexpected(&fix, &half);
    }
}

fn test_python_archive_is_rejected_before_apt() {
    let fix = Fixture::new();
    let mut members = server_fixture_files();
    members.remove("install.sh");
    members.insert(
        "install.py",
        Member::File(b"print('must-not-run')\n".to_vec()),
    );
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(out.contains("Unexpected archive contents."), "{out}");
    assert!(!out.contains("must-not-run"), "{out}");
    assert!(!fix.privileged_text().contains("apt-get"));
}

fn test_shell_archive_uses_bash_and_sqlite_not_python() {
    let fix = Fixture::new();
    let mut members = server_fixture_files();
    members.insert("install.sh", file_text(SHELL_ARCHIVE_INSTALLER));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("provisioner completed"), "{out}");
    let commands = fix.privileged_commands();
    assert!(
        !commands[3].iter().any(|arg| arg == "sqlite3"),
        "{:?}",
        commands[3]
    );
    assert!(
        !commands[3].iter().any(|arg| arg == "python3"),
        "{:?}",
        commands[3]
    );
    assert_eq!(commands[4][0], "bash");
    assert!(!fix.privileged_text().contains("disable --now"));
}

fn test_existing_shell_archive_skips_apt() {
    let mut fix = Fixture::new();
    fs::create_dir(fix.root.join("etc-kiln")).expect("etc-kiln");
    fix.env.insert("ROUTE".into(), String::new());
    let mut members = server_fixture_files();
    members.insert("install.sh", file_text(APPLY_UPDATED));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("updated"), "{out}");
    let commands = fix.privileged_commands();
    assert_eq!(commands[0], strings(&["true"]));
    assert_eq!(commands[1][0], "sh");
    assert!(
        commands[1].join(" ").contains("disable --now"),
        "{:?}",
        commands[1]
    );
    assert!(
        !commands[1].join(" ").contains("enable --now"),
        "{:?}",
        commands[1]
    );
    assert_eq!(commands[2][0], "bash");
    assert_eq!(commands.len(), 3);
    assert!(!fix.privileged_text().contains("sqlite3"));
}

fn test_sqlite3_install_is_exact_and_skipped_when_present() {
    let mut fix = Fixture::new();
    let update = sqlite_update();
    let install = sqlite_install();
    let text = install_text();
    assert!(text.contains("DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update"));
    assert!(text.contains(
        "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3"
    ));
    assert!(!text.contains("apt-get upgrade"));
    assert!(!text.contains("dist-upgrade"));
    let mut members = server_fixture_files();
    members.insert("install.sh", file_text(PROVISIONER_COMPLETED));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    let present = fix.privileged_commands();
    assert!(!present.contains(&install));
    assert!(!present.contains(&update));
    assert!(
        present
            .iter()
            .any(|command| ends_with(command, &["apt-get", "update"]))
    );
    assert!(!apt_subcommands(&present).iter().any(|sub| sub == "upgrade"));
    assert!(
        !apt_subcommands(&present)
            .iter()
            .any(|sub| sub == "dist-upgrade")
    );

    fs::remove_file(fix.root.join("privileged")).expect("clear privileged");
    fix.env.insert("PATH".into(), fix.path_without("sqlite3"));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    let missing = fix.privileged_commands();
    assert_eq!(
        missing
            .iter()
            .filter(|command| *command == &install)
            .count(),
        1
    );
    let update_at = missing
        .iter()
        .position(|command| command == &update)
        .expect("sqlite update");
    let install_at = missing
        .iter()
        .position(|command| command == &install)
        .expect("sqlite install");
    assert!(update_at < install_at);
    let general = missing
        .iter()
        .find(|command| command.last().map(String::as_str) == Some("passwd"))
        .expect("general apt install");
    assert!(!general.iter().any(|arg| arg == "sqlite3"));
    assert!(!general.iter().any(|arg| arg == "python3"));
    assert!(!apt_subcommands(&missing).iter().any(|sub| sub == "upgrade"));
    assert!(
        !apt_subcommands(&missing)
            .iter()
            .any(|sub| sub == "dist-upgrade")
    );

    fs::create_dir(fix.root.join("etc-kiln")).expect("etc-kiln");
    fs::remove_file(fix.root.join("privileged")).expect("clear privileged");
    fix.env.insert("ROUTE".into(), String::new());
    members.insert("install.sh", file_text(APPLY_UPDATED));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    let existing = fix.privileged_commands();
    assert_eq!(existing.len(), 5, "{existing:?}");
    assert_eq!(existing[0], strings(&["true"]));
    assert_eq!(existing[1], update);
    assert_eq!(existing[2], install);
    assert_eq!(existing[3][0], "sh");
    assert!(
        existing[3].join(" ").contains("disable --now"),
        "{:?}",
        existing[3]
    );
    assert_eq!(existing[4][0], "bash");
    assert_eq!(existing[4].last().map(String::as_str), Some("--apply"));
    assert!(
        !apt_subcommands(&existing)
            .iter()
            .any(|sub| sub == "upgrade")
    );
    assert!(
        !apt_subcommands(&existing)
            .iter()
            .any(|sub| sub == "dist-upgrade")
    );
}

fn test_sqlite3_apt_failure_does_not_run_the_installer() {
    let mut fix = Fixture::new();
    fs::create_dir(fix.root.join("etc-kiln")).expect("etc-kiln");
    fix.env.insert("ROUTE".into(), String::new());
    fix.env.insert("APT_FAIL".into(), "update".into());
    fix.env.insert("PATH".into(), fix.path_without("sqlite3"));
    let mut members = server_fixture_files();
    members.insert("install.sh", file_text(UPDATED_BARE));
    let digest = fix.package(&members);
    let (code, out) = fix.run(&digest, &["server"]);
    assert!(code != 0, "{out}");
    assert!(!out.contains("updated"), "{out}");
    let commands = fix.privileged_commands();
    assert!(
        commands
            .iter()
            .any(|command| ends_with(command, &["apt-get", "update"]))
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.first().map(String::as_str) == Some("bash"))
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.last().map(String::as_str) == Some("sqlite3"))
    );
}

fn test_kvm_access_is_probed_before_apt() {
    let fix = Fixture::new();
    let digest = fix.package_server();
    let (code, out) = fix.run(&digest, &["server"]);
    assert_eq!(code, 0, "{out}");
    let commands = fix.privileged_commands();
    let kvm = commands
        .iter()
        .position(|command| command.iter().any(|arg| arg.contains("kvm")))
        .expect("kvm probe");
    let update = commands
        .iter()
        .position(|command| ends_with(command, &["apt-get", "update"]))
        .expect("apt update");
    assert!(kvm < update);
}

fn test_wrong_binary_version_is_not_installed() {
    let fix = Fixture::new();
    let mut archive = Archive::new();
    archive.insert(
        "kiln",
        Member::File(b"#!/bin/sh\necho 'kiln 9.9.9'\n".to_vec()),
    );
    let digest = fix.package(&archive);
    let (code, out) = fix.run(&digest, &[]);
    assert!(code != 0, "{out}");
    assert!(out.contains("version"), "{out}");
    assert!(!fix.client_binary().exists());
}

pub fn cli(_args: &[String]) -> i32 {
    crate::run_cases(&cases())
}

register_cases! {
    test_pinned_version_matches_published_release,
    test_client_platforms_install_correct_asset,
    test_bad_digest_never_installs,
    test_http_error_never_installs,
    test_unsafe_archives_are_rejected_even_with_correct_digest,
    test_existing_client_is_preserved_without_downloading,
    test_platform_and_glibc_rejections_do_not_prompt_or_download,
    test_rerun_replaces_client_and_keeps_previous_and_profiles,
    test_failed_upgrade_preserves_current_and_previous_clients,
    test_upgrade_rejects_symlinks_and_busy_lock_before_downloading,
    test_upgrade_failed_publication_preserves_working_client,
    test_legacy_upgrade_flag_is_an_optional_client_alias,
    test_newer_client_is_not_downgraded,
    test_multiline_client_version_is_refused_before_download,
    test_existing_server_skips_apt_and_route_detection,
    test_server_unsupported_host_never_requests_privileges,
    test_ubuntu_2604_reaches_server_provisioning,
    test_debian_13_reaches_server_provisioning,
    test_server_detects_address_and_installs_without_tty,
    test_kvm_access_is_checked_by_privileged_provisioner,
    test_network_auto_detection_and_explicit_overrides,
    test_invalid_addresses_do_not_run_sudo,
    test_bad_route_fails_without_mutations,
    test_explicit_address_works_without_default_route,
    test_sudo_failure_does_not_download_or_install,
    test_bad_options_never_download_or_provision,
    test_network_environment_override_and_missing_helper,
    test_missing_or_ambiguous_network_device_requires_override,
    test_resource_limits_stop_install_before_apt,
    test_low_disk_stops_install_before_apt,
    test_bootstrap_leaves_port_binding_to_the_installer,
    test_app_units_are_required_together,
    test_app_site_and_runbook_ship_as_a_pair,
    test_door_proxy_files_match_the_packager_or_are_absent_together,
    test_pinned_archive_names_pass_and_the_caddy_pair_is_independent,
    test_python_archive_is_rejected_before_apt,
    test_shell_archive_uses_bash_and_sqlite_not_python,
    test_existing_shell_archive_skips_apt,
    test_sqlite3_install_is_exact_and_skipped_when_present,
    test_sqlite3_apt_failure_does_not_run_the_installer,
    test_kvm_access_is_probed_before_apt,
    test_wrong_binary_version_is_not_installed,
}
