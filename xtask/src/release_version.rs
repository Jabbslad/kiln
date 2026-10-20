use crate::register_cases;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const COMPONENT_LIMIT: i64 = 1_000_000_000;
const TAG_PATTERN: &str = r"^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$";
const ASSET_BEGIN: &str = "# BEGIN RELEASE ASSETS";
const ASSET_END: &str = "# END RELEASE ASSETS";
const LOCK_PACKAGE: &str = "[[package]]";

const CLAP_COMMANDS: &[(&str, &str)] = &[
    ("kiln", "kiln"),
    ("kiln-runtime", "kiln-runtime"),
    ("kiln-host", "kiln-server"),
    ("kiln-api", "kiln-server"),
    ("kiln-identity", "kiln-identity"),
];

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(TAG_PATTERN).expect("tag pattern"));
static VERSION_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^version = "([^"]*)"$"#).expect("version line"));
static NAME_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^name = "([^"]*)"$"#).expect("name line"));
static SOURCE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^source = "([^"]*)"$"#).expect("source line"));
static TABLE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([^\[\]]+)\]$").expect("table line"));
static PACKAGE_TABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[package\]$").expect("package table"));
static VERSION_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^version(\s|=|\.|$)").expect("version key"));
static VERSION_PIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^    version=[0-9]+\.[0-9]+\.[0-9]+$").expect("version pin"));
static ASSET_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^:\s]+):(\S+) ([0-9a-f]{64})$").expect("asset line"));
static HEX64_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("hex digest"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseVersion {
    major: i64,
    minor: i64,
    patch: i64,
}

impl ReleaseVersion {
    pub fn try_new(major: i64, minor: i64, patch: i64) -> Result<Self, String> {
        for (name, value) in [("major", major), ("minor", minor), ("patch", patch)] {
            if !(0..COMPONENT_LIMIT).contains(&value) {
                return Err(format!("{name} must be an int in 0 .. {COMPONENT_LIMIT}"));
            }
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    pub fn tag(&self) -> String {
        format!("v{}.{}.{}", self.major, self.minor, self.patch)
    }

    pub fn cargo(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub struct MemberCrate {
    pub name: String,
    pub inherits_workspace_version: bool,
}

pub fn parse_release_tag(raw: &str) -> Result<ReleaseVersion, String> {
    let Some(caps) = TAG_RE.captures(raw) else {
        return Err(format!(
            "refusing release tag {raw:?}: expected {TAG_PATTERN}"
        ));
    };
    ReleaseVersion::try_new(
        parse_component(&caps[1])?,
        parse_component(&caps[2])?,
        parse_component(&caps[3])?,
    )
}

pub fn plan_stamp(
    manifest: &str,
    lock: &str,
    members: &[MemberCrate],
    version: &ReleaseVersion,
) -> Result<(String, String), String> {
    if split_lines(manifest)
        .iter()
        .any(|line| PACKAGE_TABLE_RE.is_match(line))
    {
        return Err("root Cargo.toml has a [package] table".to_string());
    }
    let parsed = loads(manifest, "Cargo.toml")?;
    let declared = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(|members| members.as_array());
    let Some(declared) = declared else {
        return Err("member list does not match workspace.members".to_string());
    };
    if declared.len() != members.len() {
        return Err("member list does not match workspace.members".to_string());
    }
    let mut seen = BTreeSet::new();
    for member in members {
        if !seen.insert(member.name.as_str()) {
            return Err("duplicate workspace member name".to_string());
        }
    }
    for member in members {
        if !member.inherits_workspace_version {
            return Err(format!(
                "{} does not inherit the workspace version",
                member.name
            ));
        }
    }
    let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
    let cargo = version.cargo();
    let new_manifest = rewrite_manifest(manifest, &cargo)?;
    let new_lock = rewrite_lock(lock, &names, &cargo)?;
    postcondition(lock, &new_manifest, &new_lock, &names, version)?;
    Ok((new_manifest, new_lock))
}

pub fn resolve(repo: &Path, ref_type: &str, ref_name: &str) -> Result<ReleaseVersion, String> {
    if ref_type == "branch" {
        let text = read_text(&repo.join("Cargo.toml"))?;
        return committed_version(&text);
    }
    if ref_type != "tag" {
        return Err(format!("refusing ref_type {ref_type:?}"));
    }
    let version = parse_release_tag(ref_name)?;
    let manifest_path = repo.join("Cargo.toml");
    let lock_path = repo.join("Cargo.lock");
    let manifest_bytes = read_bytes(&manifest_path)?;
    let lock_bytes = read_bytes(&lock_path)?;
    let manifest = utf8_text(&manifest_path, &manifest_bytes)?;
    let lock = utf8_text(&lock_path, &lock_bytes)?;
    let members = load_members(repo, &manifest)?;
    let (new_manifest, new_lock) = plan_stamp(&manifest, &lock, &members, &version)?;
    replace_bytes(&lock_path, &lock_bytes, new_lock.as_bytes())?;
    replace_bytes(&manifest_path, &manifest_bytes, new_manifest.as_bytes())?;
    Ok(version)
}

pub fn clap_line(binary: &Path, version: &ReleaseVersion) -> Result<String, String> {
    let name = file_name(binary);
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    let command = CLAP_COMMANDS
        .iter()
        .find(|(key, _)| *key == stem)
        .map(|(_, command)| *command)
        .ok_or_else(|| format!("unknown binary {name}"))?;
    Ok(format!("{command} {}", version.cargo()))
}

pub fn check_binary(version: &ReleaseVersion, binaries: &[PathBuf]) -> Result<(), String> {
    if binaries.is_empty() {
        return Err("no binaries to check".to_string());
    }
    for binary in binaries {
        if not_regular_file(binary) {
            return Err(format!("refusing non-regular binary {}", binary.display()));
        }
        let expected = format!("{}\n", clap_line(binary, version)?);
        let mut command = Command::new(binary);
        command.arg("--version");
        let output = match capture(command, None, Duration::from_secs(30)) {
            Ok(output) => output,
            Err(RunError::Start) => {
                return Err(format!("{} --version failed to start", binary.display()));
            }
            Err(RunError::Timeout) => {
                return Err(format!("{} --version timed out", binary.display()));
            }
        };
        let stdout = normalize_crlf(&output.stdout);
        let expected_bytes = expected.as_bytes();
        if !output.status.success() || stdout != expected_bytes {
            return Err(format!(
                "{} --version printed {}, expected {}",
                binary.display(),
                py_bytes(&output.stdout),
                py_bytes(expected_bytes),
            ));
        }
    }
    Ok(())
}

pub fn check_archives(version: &ReleaseVersion, dist: &Path) -> Result<(), String> {
    if !dist.is_dir() {
        return Err(format!("{} is not a directory", dist.display()));
    }
    let pattern = dist_pattern(version)?;
    let mut names = Vec::new();
    let entries = fs::read_dir(dist).map_err(|err| err.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        let name = file_name(&path);
        if not_regular_file(&path) || !pattern.is_match(&name) {
            return Err(format!("unexpected dist file {name}"));
        }
        names.push(name);
    }
    let archives: Vec<&str> = names
        .iter()
        .filter(|name| !name.ends_with(".sha256"))
        .map(String::as_str)
        .collect();
    let checksums: BTreeSet<&str> = names
        .iter()
        .filter(|name| name.ends_with(".sha256"))
        .map(String::as_str)
        .collect();
    if archives.is_empty() {
        return Err(format!("{} has no archives", dist.display()));
    }
    let archive_set: BTreeSet<&str> = archives.iter().copied().collect();
    for name in &archives {
        let sidecar = format!("{name}.sha256");
        if !checksums.contains(sidecar.as_str()) {
            return Err(format!("missing checksum for {name}"));
        }
    }
    for name in &checksums {
        let archive = name.strip_suffix(".sha256").unwrap_or(*name);
        if !archive_set.contains(archive) {
            return Err(format!("checksum without archive {name}"));
        }
    }
    let tag = version.tag();
    for name in archives {
        let Some(payload) = release_json(&dist.join(name))? else {
            continue;
        };
        if payload.get("version").and_then(serde_json::Value::as_str) != Some(tag.as_str()) {
            return Err(format!(
                "{name} release.json version is {}, expected {tag}",
                py_json_repr(payload.get("version")),
            ));
        }
    }
    Ok(())
}

pub fn render_bootstrap(
    source: &str,
    version: &ReleaseVersion,
    digests: &HashMap<String, String>,
) -> Result<String, String> {
    validate_digests(digests)?;
    let mut lines: Vec<String> = split_lines_keep(source)
        .into_iter()
        .map(str::to_string)
        .collect();
    let pins: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| VERSION_PIN_RE.is_match(body(line)))
        .map(|(index, _)| index)
        .collect();
    if pins.len() != 1 {
        return Err("install.sh needs exactly one version pin".to_string());
    }
    let rows = asset_rows(&lines)?;
    let row_keys: BTreeSet<&str> = rows.iter().map(|(_, key)| key.as_str()).collect();
    let supplied: BTreeSet<&str> = digests.keys().map(String::as_str).collect();
    if row_keys != supplied {
        return Err("release asset keys do not match the supplied digests".to_string());
    }
    let pin = pins[0];
    let cargo = version.cargo();
    let pin_ending = ending(&lines[pin]).to_string();
    lines[pin] = format!("    version={cargo}{pin_ending}");
    for (index, key) in &rows {
        let prefix = {
            let current = body(&lines[*index]);
            let Some((prefix, _)) = current.rsplit_once(' ') else {
                return Err(format!("invalid release asset line {current:?}"));
            };
            prefix.to_string()
        };
        let line_ending = ending(&lines[*index]).to_string();
        let Some(digest) = digests.get(key) else {
            return Err(format!(
                "release digest for {key:?} must be 64 lowercase hex"
            ));
        };
        lines[*index] = format!("{prefix} {digest}{line_ending}");
    }
    Ok(lines.concat())
}

pub fn attach_bootstrap(
    repo: &Path,
    dist: &Path,
    version: &ReleaseVersion,
) -> Result<PathBuf, String> {
    let source_path = repo.join("deploy/bootstrap/install.sh");
    let source = read_text(&source_path)?;
    let lines: Vec<String> = split_lines_keep(&source)
        .into_iter()
        .map(str::to_string)
        .collect();
    let rows = asset_rows(&lines)?;
    let mut digests = HashMap::new();
    for (_, key) in &rows {
        let archive = archive_name(key, version)?;
        let digest = sidecar_digest(&dist.join(format!("{archive}.sha256")))?;
        digests.insert(key.clone(), digest);
    }
    let rendered = render_bootstrap(&source, version, &digests)?;
    let data = rendered.into_bytes();
    let script = dist.join("install.sh");
    write_file(&script, &data, Some(0o755))?;
    let digest = sha256_hex(&data);
    write_file(
        &dist.join("install.sh.sha256"),
        format!("{digest}  install.sh\n").as_bytes(),
        None,
    )?;
    Ok(script)
}

pub fn committed_version(manifest: &str) -> Result<ReleaseVersion, String> {
    let parsed = loads(manifest, "Cargo.toml")?;
    let cargo = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .ok_or_else(|| "Cargo.toml has no workspace package version".to_string())?;
    let cargo = cargo
        .as_str()
        .ok_or_else(|| "workspace package version is not a string".to_string())?;
    parse_release_tag(&format!("v{cargo}"))
}

pub fn dispatch(args: &[String], env: &HashMap<String, String>) -> Result<String, String> {
    let args = if args.first().is_some_and(|arg| arg == "--") {
        &args[1..]
    } else {
        args
    };
    let Some(command) = args.first() else {
        return Err("a command is required".to_string());
    };
    match command.as_str() {
        "resolve" => dispatch_resolve(&args[1..], env),
        "check-binary" => dispatch_check_binary(&args[1..]),
        "check-archives" => dispatch_check_archives(&args[1..]),
        "attach-bootstrap" => dispatch_attach(&args[1..]),
        other => Err(format!("unknown command {other}")),
    }
}

pub fn cli(args: &[String]) -> i32 {
    let env: HashMap<String, String> = std::env::vars().collect();
    match dispatch(args, &env) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn dispatch_resolve(args: &[String], env: &HashMap<String, String>) -> Result<String, String> {
    let parsed = parse_flags(args, &["--ref-type", "--ref-name", "--repo"])?;
    if !parsed.positionals.is_empty() {
        return Err(format!(
            "unrecognized arguments: {}",
            parsed.positionals.join(" ")
        ));
    }
    let ref_type = required(&parsed.options, "--ref-type")?;
    let ref_name = required(&parsed.options, "--ref-name")?;
    let repo = match parsed.options.get("--repo") {
        Some(path) => PathBuf::from(path),
        None => crate::repo_root(),
    };
    let version = resolve(&repo, ref_type, ref_name)?;
    if let Some(path) = env.get("GITHUB_ENV").filter(|path| !path.is_empty()) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|err| err.to_string())?;
        writeln!(file, "VERSION={}", version.tag()).map_err(|err| err.to_string())?;
    }
    Ok(format!("{}\n", version.tag()))
}

fn dispatch_check_binary(args: &[String]) -> Result<String, String> {
    let parsed = parse_flags(args, &["--version"])?;
    let raw = required(&parsed.options, "--version")?;
    if parsed.positionals.is_empty() {
        return Err("the following arguments are required: binaries".to_string());
    }
    let version = parse_release_tag(raw)?;
    let binaries: Vec<PathBuf> = parsed.positionals.iter().map(PathBuf::from).collect();
    check_binary(&version, &binaries)?;
    Ok(String::new())
}

fn dispatch_check_archives(args: &[String]) -> Result<String, String> {
    let parsed = parse_flags(args, &["--version"])?;
    let raw = required(&parsed.options, "--version")?;
    if parsed.positionals.len() != 1 {
        return Err("dist argument is required".to_string());
    }
    let version = parse_release_tag(raw)?;
    check_archives(&version, Path::new(&parsed.positionals[0]))?;
    Ok(String::new())
}

fn dispatch_attach(args: &[String]) -> Result<String, String> {
    let parsed = parse_flags(args, &["--version", "--dist", "--repo"])?;
    if !parsed.positionals.is_empty() {
        return Err(format!(
            "unrecognized arguments: {}",
            parsed.positionals.join(" ")
        ));
    }
    let raw = required(&parsed.options, "--version")?;
    let dist = required(&parsed.options, "--dist")?;
    let repo = match parsed.options.get("--repo") {
        Some(path) => PathBuf::from(path),
        None => crate::repo_root(),
    };
    let version = parse_release_tag(raw)?;
    attach_bootstrap(&repo, Path::new(dist), &version)?;
    Ok(String::new())
}

struct FlagArgs {
    options: HashMap<String, String>,
    positionals: Vec<String>,
}

fn parse_flags(args: &[String], known: &[&str]) -> Result<FlagArgs, String> {
    let mut options = HashMap::new();
    let mut positionals = Vec::new();
    let mut index = 0;
    let mut only_positionals = false;
    while index < args.len() {
        let arg = &args[index];
        if !only_positionals && arg == "--" {
            only_positionals = true;
            index += 1;
            continue;
        }
        if !only_positionals && arg.starts_with("--") {
            let (key, inline) = match arg.split_once('=') {
                Some((key, value)) => (key.to_string(), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            if !known.contains(&key.as_str()) {
                return Err(format!("unrecognized arguments: {arg}"));
            }
            let value = if let Some(value) = inline {
                value
            } else {
                index += 1;
                args.get(index)
                    .cloned()
                    .ok_or_else(|| format!("argument {key}: expected one argument"))?
            };
            options.insert(key, value);
            index += 1;
            continue;
        }
        positionals.push(arg.clone());
        index += 1;
    }
    Ok(FlagArgs {
        options,
        positionals,
    })
}

fn required<'a>(options: &'a HashMap<String, String>, key: &str) -> Result<&'a str, String> {
    options
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("the following arguments are required: {key}"))
}

fn parse_component(text: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|_| format!("component must be an int in 0 .. {COMPONENT_LIMIT}"))
}

fn load_members(repo: &Path, manifest: &str) -> Result<Vec<MemberCrate>, String> {
    let parsed = loads(manifest, "Cargo.toml")?;
    let rels = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .ok_or_else(|| "Cargo.toml has no workspace members".to_string())?;
    let rels = rels
        .as_array()
        .ok_or_else(|| "workspace.members is not a list of paths".to_string())?;
    if rels.iter().any(|rel| rel.as_str().is_none()) {
        return Err("workspace.members is not a list of paths".to_string());
    }
    let mut members = Vec::new();
    let mut seen = BTreeSet::new();
    for rel_value in rels {
        let rel = rel_value.as_str().unwrap_or("");
        let path = repo.join(rel).join("Cargo.toml");
        if !path.is_file() {
            return Err(format!("missing member manifest: {rel}"));
        }
        let member = loads(&read_text(&path)?, &path.display().to_string())?;
        let name_val = member
            .get("package")
            .and_then(|package| package.get("name"));
        let version_val = member
            .get("package")
            .and_then(|package| package.get("version"));
        let (Some(name_val), Some(version_val)) = (name_val, version_val) else {
            return Err(format!("{rel} has no package name and version"));
        };
        let Some(name) = name_val.as_str() else {
            return Err(format!("duplicate package name {name_val:?}"));
        };
        if name.is_empty() || seen.contains(name) {
            return Err(format!("duplicate package name {name:?}"));
        }
        seen.insert(name.to_string());
        members.push(MemberCrate {
            name: name.to_string(),
            inherits_workspace_version: inherits_workspace(version_val),
        });
    }
    Ok(members)
}

fn inherits_workspace(version: &toml::Value) -> bool {
    let Some(table) = version.as_table() else {
        return false;
    };
    table.len() == 1 && table.get("workspace").and_then(toml::Value::as_bool) == Some(true)
}

fn rewrite_manifest(manifest: &str, cargo: &str) -> Result<String, String> {
    let lines = split_lines_keep(manifest);
    let mut section = String::new();
    let mut matches = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let text = body(line);
        if let Some(caps) = TABLE_LINE_RE.captures(text) {
            section = caps[1].to_string();
            continue;
        }
        if section != "workspace.package" {
            continue;
        }
        if VERSION_LINE_RE.is_match(text) {
            matches.push(index);
            continue;
        }
        if VERSION_KEY_RE.is_match(text) {
            return Err("workspace.package version is not a simple assignment".to_string());
        }
    }
    if matches.len() != 1 {
        return Err(
            "Cargo.toml [workspace.package] needs exactly one version assignment".to_string(),
        );
    }
    let index = matches[0];
    let mut out = String::with_capacity(manifest.len());
    for (line_index, line) in lines.iter().enumerate() {
        if line_index == index {
            out.push_str(&format!("version = \"{cargo}\"{}", ending(line)));
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

struct BlockInfo {
    name: String,
    source: Option<String>,
    version_lines: usize,
}

fn rewrite_lock(lock: &str, names: &[&str], cargo: &str) -> Result<String, String> {
    let (header, blocks) = split_lock(lock)?;
    if !split_lines_keep(&header)
        .iter()
        .any(|line| body(line) == "version = 4")
    {
        return Err("Cargo.lock header version is not 4".to_string());
    }
    let mut infos = Vec::with_capacity(blocks.len());
    for block in &blocks {
        infos.push(inspect_block(block)?);
    }
    let mut hits: BTreeMap<&str, Vec<usize>> =
        names.iter().map(|name| (*name, Vec::new())).collect();
    for (index, info) in infos.iter().enumerate() {
        let is_path = match &info.source {
            None => true,
            Some(source) => source.starts_with("path+"),
        };
        if is_path {
            if let Some(slot) = hits.get_mut(info.name.as_str()) {
                slot.push(index);
            }
        }
    }
    let mut replaced = BTreeMap::new();
    for name in names {
        let indexes = &hits[*name];
        if indexes.len() != 1 {
            return Err(format!(
                "Cargo.lock has no path package for {name}. Refusing to run cargo update."
            ));
        }
        let index = indexes[0];
        if infos[index].version_lines != 1 {
            return Err(format!(
                "Cargo.lock path package {name} needs exactly one version assignment"
            ));
        }
        replaced.insert(index, replace_version_line(&blocks[index], cargo)?);
    }
    let mut out = header;
    for (index, block) in blocks.iter().enumerate() {
        if let Some(rewritten) = replaced.get(&index) {
            out.push_str(rewritten);
        } else {
            out.push_str(block);
        }
    }
    Ok(out)
}

fn inspect_block(block: &str) -> Result<BlockInfo, String> {
    let mut name = None;
    let mut source = None;
    let mut version_lines = 0usize;
    for line in split_lines(block) {
        if let Some(caps) = NAME_LINE_RE.captures(line) {
            if name.is_some() {
                return Err("Cargo.lock package block has two names".to_string());
            }
            name = Some(caps[1].to_string());
            continue;
        }
        if let Some(caps) = SOURCE_LINE_RE.captures(line) {
            if source.is_some() {
                return Err(format!(
                    "Cargo.lock package {} has two sources",
                    display_name(&name)
                ));
            }
            source = Some(caps[1].to_string());
            continue;
        }
        if line.starts_with("source") {
            return Err(format!(
                "Cargo.lock package {} has an unreadable source",
                display_name(&name)
            ));
        }
        if VERSION_LINE_RE.is_match(line) {
            version_lines += 1;
        } else if line.starts_with("version") {
            return Err(format!(
                "Cargo.lock package {} has an unreadable version",
                display_name(&name)
            ));
        }
    }
    let Some(name) = name.filter(|name| !name.is_empty()) else {
        return Err("Cargo.lock package block has no name".to_string());
    };
    Ok(BlockInfo {
        name,
        source,
        version_lines,
    })
}

fn display_name(name: &Option<String>) -> &str {
    name.as_deref().unwrap_or("None")
}

fn replace_version_line(block: &str, cargo: &str) -> Result<String, String> {
    let lines = split_lines_keep(block);
    let found: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| VERSION_LINE_RE.is_match(body(line)))
        .map(|(index, _)| index)
        .collect();
    if found.len() != 1 {
        return Err("Cargo.lock path package needs exactly one version assignment".to_string());
    }
    let index = found[0];
    let mut out = String::with_capacity(block.len());
    for (line_index, line) in lines.iter().enumerate() {
        if line_index == index {
            out.push_str(&format!("version = \"{cargo}\"{}", ending(line)));
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

fn split_lock(lock: &str) -> Result<(String, Vec<String>), String> {
    let lines = split_lines_keep(lock);
    let mut header = Vec::new();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in lines {
        if body(line) == LOCK_PACKAGE {
            if let Some(done) = current.take() {
                blocks.push(done);
            }
            current = Some(vec![line]);
            continue;
        }
        if let Some(block) = current.as_mut() {
            block.push(line);
        } else {
            header.push(line);
        }
    }
    if let Some(done) = current {
        blocks.push(done);
    }
    if blocks.is_empty() {
        return Err("Cargo.lock has no packages".to_string());
    }
    Ok((
        header.concat(),
        blocks.into_iter().map(|block| block.concat()).collect(),
    ))
}

fn postcondition(
    old_lock: &str,
    new_manifest: &str,
    new_lock: &str,
    names: &[&str],
    version: &ReleaseVersion,
) -> Result<(), String> {
    if lock_header(new_lock)? != lock_header(old_lock)? {
        return Err("Cargo.lock header changed".to_string());
    }
    let manifest = loads(new_manifest, "stamped Cargo.toml")?;
    let stamped = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .ok_or_else(|| "stamped Cargo.toml has no workspace package version".to_string())?;
    let cargo = version.cargo();
    if stamped.as_str() != Some(cargo.as_str()) {
        return Err("stamped workspace version does not match the tag".to_string());
    }
    let old_doc = loads(old_lock, "Cargo.lock")?;
    let new_doc = loads(new_lock, "stamped Cargo.lock")?;
    if old_doc.get("version").and_then(toml::Value::as_integer) != Some(4)
        || new_doc.get("version").and_then(toml::Value::as_integer) != Some(4)
    {
        return Err("Cargo.lock header version is not 4".to_string());
    }
    let old_packages = old_doc
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "Cargo.lock has no packages".to_string())?;
    let new_packages = new_doc
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "Cargo.lock has no packages".to_string())?;
    if old_packages.len() != new_packages.len() {
        return Err("Cargo.lock package count changed".to_string());
    }
    let wanted: BTreeSet<&str> = names.iter().copied().collect();
    let mut seen: BTreeMap<&str, usize> = names.iter().map(|name| (*name, 0)).collect();
    for (old_pkg, new_pkg) in old_packages.iter().zip(new_packages.iter()) {
        let (Some(old_pkg), Some(new_pkg)) = (old_pkg.as_table(), new_pkg.as_table()) else {
            return Err("Cargo.lock package table is unreadable".to_string());
        };
        if old_pkg.get("name") != new_pkg.get("name") {
            return Err("Cargo.lock package name changed".to_string());
        }
        let name = match new_pkg.get("name").and_then(toml::Value::as_str) {
            Some(name) => name,
            None => "None",
        };
        if old_pkg.get("dependencies") != new_pkg.get("dependencies") {
            return Err(format!("Cargo.lock dependencies changed for {name}"));
        }
        if old_pkg.get("source") != new_pkg.get("source") {
            return Err(format!("Cargo.lock source changed for {name}"));
        }
        let source = new_pkg.get("source").and_then(toml::Value::as_str);
        let source_missing = new_pkg.get("source").is_none();
        let is_path = source_missing || source.is_some_and(|value| value.starts_with("path+"));
        let is_foreign =
            source.is_some_and(|value| value.starts_with("registry+") || value.starts_with("git+"));
        if is_foreign && new_pkg.get("version") != old_pkg.get("version") {
            return Err(format!("Cargo.lock changed registry or git package {name}"));
        }
        if is_path && wanted.contains(name) {
            if new_pkg.get("version").and_then(toml::Value::as_str) != Some(cargo.as_str()) {
                return Err(format!("Cargo.lock path package {name} is not {cargo}"));
            }
            if let Some(count) = seen.get_mut(name) {
                *count += 1;
            }
        } else if new_pkg.get("version") != old_pkg.get("version") {
            return Err(format!("Cargo.lock changed package {name}"));
        }
    }
    for (name, count) in seen {
        if count != 1 {
            return Err(format!(
                "Cargo.lock has no path package for {name}. Refusing to run cargo update."
            ));
        }
    }
    Ok(())
}

enum ArchiveRead {
    Invalid(String),
    Unreadable,
}

fn release_json(path: &Path) -> Result<Option<serde_json::Value>, String> {
    let name = file_name(path);
    let read = if name.ends_with(".tar.gz") {
        tar_release_json(path, &name)
    } else if name.ends_with(".zip") {
        zip_release_json(path, &name)
    } else {
        return Err(format!("unexpected archive {name}"));
    };
    match read {
        Ok(None) => Ok(None),
        Ok(Some(value)) if value.is_object() => Ok(Some(value)),
        Ok(Some(_)) => Err(format!("{name} release.json is not an object")),
        Err(ArchiveRead::Invalid(message)) => Err(message),
        Err(ArchiveRead::Unreadable) => Err(format!("{name} is not a readable package")),
    }
}

fn tar_release_json(
    path: &Path,
    archive_name: &str,
) -> Result<Option<serde_json::Value>, ArchiveRead> {
    let file = File::open(path).map_err(|_| ArchiveRead::Unreadable)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|_| ArchiveRead::Unreadable)?;
    for entry in entries {
        let mut entry = entry.map_err(|_| ArchiveRead::Unreadable)?;
        let entry_path = entry
            .path()
            .map_err(|_| ArchiveRead::Unreadable)?
            .into_owned();
        if entry_path != Path::new("release.json") {
            continue;
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind == tar::EntryType::Link || !kind.is_file() {
            return Err(ArchiveRead::Invalid(format!(
                "{archive_name} release.json is not a regular file"
            )));
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|_| ArchiveRead::Unreadable)?;
        let payload = serde_json::from_slice(&data).map_err(|_| ArchiveRead::Unreadable)?;
        return Ok(Some(payload));
    }
    Ok(None)
}

fn zip_release_json(
    path: &Path,
    archive_name: &str,
) -> Result<Option<serde_json::Value>, ArchiveRead> {
    let file = File::open(path).map_err(|_| ArchiveRead::Unreadable)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| ArchiveRead::Unreadable)?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| ArchiveRead::Unreadable)?;
        if entry.name() != "release.json" {
            continue;
        }
        if entry.is_dir() {
            return Err(ArchiveRead::Invalid(format!(
                "{archive_name} release.json is not a regular file"
            )));
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|_| ArchiveRead::Unreadable)?;
        let payload = serde_json::from_slice(&data).map_err(|_| ArchiveRead::Unreadable)?;
        return Ok(Some(payload));
    }
    Ok(None)
}

fn validate_digests(digests: &HashMap<String, String>) -> Result<(), String> {
    for (key, digest) in digests {
        if !HEX64_RE.is_match(digest) {
            return Err(format!(
                "release digest for {key:?} must be 64 lowercase hex"
            ));
        }
    }
    Ok(())
}

fn asset_rows(lines: &[String]) -> Result<Vec<(usize, String)>, String> {
    let begin: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| body(line) == ASSET_BEGIN)
        .map(|(index, _)| index)
        .collect();
    let end: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| body(line) == ASSET_END)
        .map(|(index, _)| index)
        .collect();
    if begin.len() != 1 || end.len() != 1 || begin[0] >= end[0] {
        return Err("install.sh release asset markers are missing or duplicated".to_string());
    }
    let mut rows = Vec::new();
    for index in (begin[0] + 1)..end[0] {
        let text = body(&lines[index]);
        if text.trim_start().starts_with('#') {
            continue;
        }
        let Some(caps) = ASSET_LINE_RE.captures(text) else {
            return Err(format!("invalid release asset line {text:?}"));
        };
        let key = format!("{}:{}", &caps[1], &caps[2]);
        if rows.iter().any(|(_, existing)| existing == &key) {
            return Err(format!("duplicate release asset {key}"));
        }
        rows.push((index, key));
    }
    Ok(rows)
}

fn archive_name(key: &str, version: &ReleaseVersion) -> Result<String, String> {
    let Some((mode, target)) = key.split_once(':') else {
        return Err(format!("unknown release asset {key}"));
    };
    if target.is_empty() {
        return Err(format!("unknown release asset {key}"));
    }
    let tag = version.tag();
    match mode {
        "client" => Ok(format!("kiln-{tag}-{target}.tar.gz")),
        "server" => Ok(format!("kiln-server-{tag}-{target}.tar.gz")),
        _ => Err(format!("unknown release asset {key}")),
    }
}

fn sidecar_digest(path: &Path) -> Result<String, String> {
    if not_regular_file(path) {
        return Err(format!("missing checksum sidecar {}", file_name(path)));
    }
    let text = read_text(path)?;
    let digest = text.split_whitespace().next().unwrap_or("");
    if !HEX64_RE.is_match(digest) {
        return Err(format!(
            "checksum sidecar {} first field is not 64 lowercase hex",
            file_name(path)
        ));
    }
    Ok(digest.to_string())
}

fn write_file(path: &Path, data: &[u8], mode: Option<u32>) -> Result<(), String> {
    let parent = parent_dir(path);
    if !parent.is_dir() {
        return Err(format!("{} is not a directory", parent.display()));
    }
    atomic_write(path, data, mode)
}

fn replace_bytes(path: &Path, old: &[u8], new: &[u8]) -> Result<(), String> {
    if old == new {
        return Ok(());
    }
    atomic_write(path, new, None)
}

fn atomic_write(path: &Path, data: &[u8], mode: Option<u32>) -> Result<(), String> {
    let parent = parent_dir(path);
    let prefix = format!(".{}.", file_name(path));
    let mut tmp = tempfile::Builder::new()
        .prefix(&prefix)
        .tempfile_in(parent)
        .map_err(|err| err.to_string())?;
    tmp.write_all(data).map_err(|err| err.to_string())?;
    tmp.flush().map_err(|err| err.to_string())?;
    tmp.as_file().sync_all().map_err(|err| err.to_string())?;
    if let Some(mode) = mode {
        set_mode(tmp.path(), mode)?;
    }
    tmp.persist(path)
        .map(|_| ())
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn dist_pattern(version: &ReleaseVersion) -> Result<Regex, String> {
    let tag = regex::escape(&version.tag());
    Regex::new(&format!(
        r"^kiln(-[A-Za-z0-9]+)*-{tag}(-[A-Za-z0-9_]+)*\.(tar\.gz|zip)(\.sha256)?$"
    ))
    .map_err(|err| err.to_string())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|_| format!("cannot read {}", path.display()))
}

fn read_text(path: &Path) -> Result<String, String> {
    utf8_text(path, &read_bytes(path)?)
}

fn utf8_text(path: &Path, bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| format!("{} is not UTF-8", path.display()))
}

fn loads(text: &str, label: &str) -> Result<toml::Value, String> {
    let parsed: toml::Value = text
        .parse()
        .map_err(|_| format!("{label} is not valid TOML"))?;
    if parsed.as_table().is_none() {
        return Err(format!("{label} is not a TOML table"));
    }
    Ok(parsed)
}

fn lock_header(lock: &str) -> Result<&str, String> {
    lock.find(LOCK_PACKAGE)
        .map(|index| &lock[..index])
        .ok_or_else(|| "Cargo.lock has no packages".to_string())
}

fn body(line: &str) -> &str {
    if let Some(stripped) = line.strip_suffix("\r\n") {
        stripped
    } else if let Some(stripped) = line.strip_suffix('\n') {
        stripped
    } else {
        line
    }
}

fn ending(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn split_lines_keep(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            index += 1;
            lines.push(&text[start..index]);
            start = index;
        } else if bytes[index] == b'\r' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                index += 2;
            } else {
                index += 1;
            }
            lines.push(&text[start..index]);
            start = index;
        } else {
            index += 1;
        }
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn split_lines(text: &str) -> Vec<&str> {
    split_lines_keep(text)
        .into_iter()
        .map(|line| {
            if let Some(stripped) = line.strip_suffix("\r\n") {
                stripped
            } else if let Some(stripped) = line.strip_suffix('\n') {
                stripped
            } else {
                line.strip_suffix('\r').unwrap_or(line)
            }
        })
        .collect()
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn not_regular_file(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let kind = meta.file_type();
            kind.is_symlink() || !kind.is_file()
        }
        Err(_) => true,
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|err| err.to_string());
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn normalize_crlf(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == b'\r' && index + 1 < data.len() && data[index + 1] == b'\n' {
            out.push(b'\n');
            index += 2;
        } else {
            out.push(data[index]);
            index += 1;
        }
    }
    out
}

fn py_bytes(data: &[u8]) -> String {
    let mut out = String::from("b'");
    for byte in data {
        match byte {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            0x20..=0x7e => out.push(*byte as char),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('\'');
    out
}

fn py_json_repr(value: Option<&serde_json::Value>) -> String {
    match value {
        None => "None".to_string(),
        Some(serde_json::Value::String(text)) => format!("'{text}'"),
        Some(other) => other.to_string(),
    }
}

#[derive(Debug)]
enum RunError {
    Start,
    Timeout,
}

struct Captured {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn capture(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Captured, RunError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().map_err(|_| RunError::Start)?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_thread = std::thread::spawn(move || read_pipe(stdout_pipe));
    let stderr_thread = std::thread::spawn(move || read_pipe(stderr_pipe));
    if let Some(data) = input {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data);
        }
    }
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(RunError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break Err(RunError::Start),
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    match status {
        Ok(status) => Ok(Captured {
            status,
            stdout,
            stderr,
        }),
        Err(error) => Err(error),
    }
}

fn read_pipe(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    buf
}

fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let _ = command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

const TAG_PATTERN_IN_ERROR: &str = "(0|[1-9][0-9]{0,8})";

const MANIFEST: &str = r#"[workspace]
members = ["crates/one", "crates/two"]
resolver = "3"

[workspace.package]
version = "0.4.6"
edition = "2024"
rust-version = "1.95"
publish = false

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
"#;

const EXPECTED_MANIFEST: &str = r#"[workspace]
members = ["crates/one", "crates/two"]
resolver = "3"

[workspace.package]
version = "9.9.9"
edition = "2024"
rust-version = "1.95"
publish = false

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
"#;

const LOCK: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "one"
version = "0.4.6"
dependencies = [
 "some-crate 0.4.6",
]

[[package]]
name = "other-path"
version = "0.4.6"

[[package]]
name = "registry-crate"
version = "0.4.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"

[[package]]
name = "git-crate"
version = "0.4.6"
source = "git+https://github.com/example/git-crate?rev=abc"

[[package]]
name = "two"
version = "0.4.6"
dependencies = [
 "registry-crate",
]
"#;

const EXPECTED_LOCK: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "one"
version = "9.9.9"
dependencies = [
 "some-crate 0.4.6",
]

[[package]]
name = "other-path"
version = "0.4.6"

[[package]]
name = "registry-crate"
version = "0.4.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"

[[package]]
name = "git-crate"
version = "0.4.6"
source = "git+https://github.com/example/git-crate?rev=abc"

[[package]]
name = "two"
version = "9.9.9"
dependencies = [
 "registry-crate",
]
"#;

const ONE: &str = r#"[package]
name = "one"
version.workspace = true
"#;

const TWO: &str = r#"[package]
name = "two"
version.workspace = true
"#;

const TRACKED: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "crates/one/Cargo.toml",
    "crates/two/Cargo.toml",
];

const LINUX_CLIENT_URL: &str = "https://github.com/Jabbslad/kiln/releases/download/v9.9.9/kiln-v9.9.9-x86_64-unknown-linux-gnu.tar.gz";
const LINUX_SERVER_URL: &str = "https://github.com/Jabbslad/kiln/releases/download/v9.9.9/kiln-server-v9.9.9-x86_64-unknown-linux-gnu.tar.gz";

const ASSET_KEYS: &[&str] = &[
    "client:x86_64-unknown-linux-gnu",
    "client:x86_64-apple-darwin",
    "client:aarch64-apple-darwin",
    "server:x86_64-unknown-linux-gnu",
];

const PACKAGE_FILES: &[&str] = &[
    "deploy/install.sh",
    "deploy/kiln-network",
    "deploy/kiln-host.service",
    "deploy/kiln-api.service",
    "deploy/kiln-door.service",
    "deploy/kiln-app.service",
    "deploy/kiln-app-proxy.service",
    "deploy/kiln-app-proxy.socket",
    "deploy/kiln-door-proxy.service",
    "deploy/kiln-door-proxy.socket",
    "deploy/paseo-caddy.override.yaml",
    "deploy/door.Caddyfile",
    "deploy/app.Caddyfile",
    "deploy/enroll-dark-forge.sh",
    "deploy/host.example.json",
    "scripts/fetch-firecracker.sh",
    "README.md",
    "docs/releases.md",
    "docs/remote-client.md",
    "docs/identity-service.md",
    "docs/runtime.md",
    "docs/runbooks/2026-10-19-public-door.md",
    "docs/runbooks/2026-10-19-door-public-cert.md",
    "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
];

struct TempRepo {
    #[allow(dead_code)]
    temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

fn temp_repo() -> TempRepo {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let repo = root.join("repo");
    fs::create_dir(&repo).unwrap();
    assert_ne!(repo.canonicalize().unwrap(), crate::repo_root());
    TempRepo { temp, root, repo }
}

fn sample_members() -> Vec<MemberCrate> {
    vec![
        MemberCrate {
            name: "one".to_string(),
            inherits_workspace_version: true,
        },
        MemberCrate {
            name: "two".to_string(),
            inherits_workspace_version: true,
        },
    ]
}

fn write_repo(root: &Path, manifest: &str, lock: &str, one: &str, two: &str) {
    fs::write(root.join("Cargo.toml"), manifest).unwrap();
    fs::write(root.join("Cargo.lock"), lock).unwrap();
    fs::create_dir_all(root.join("crates/one")).unwrap();
    fs::create_dir_all(root.join("crates/two")).unwrap();
    fs::write(root.join("crates/one/Cargo.toml"), one).unwrap();
    fs::write(root.join("crates/two/Cargo.toml"), two).unwrap();
}

fn snapshot(root: &Path) -> Vec<(Vec<u8>, SystemTime)> {
    TRACKED
        .iter()
        .map(|rel| {
            let path = root.join(rel);
            let bytes = fs::read(&path).unwrap();
            let mtime = fs::metadata(&path).unwrap().modified().unwrap();
            (bytes, mtime)
        })
        .collect()
}

fn set_stamp(path: &Path, nanos: u64) {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let when = UNIX_EPOCH + Duration::from_nanos(nanos);
    file.set_times(FileTimes::new().set_accessed(when).set_modified(when))
        .unwrap();
}

fn mtime_nanos(path: &Path) -> u128 {
    fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn dir_bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut map = BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        map.insert(file_name(&entry.path()), fs::read(entry.path()).unwrap());
    }
    map
}

fn write_named(root: &Path, names: &[&str]) {
    for name in names {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, name).unwrap();
    }
}

fn test_rejects_prerelease_build_metadata_leading_zeros_and_bare_cargo() {
    let refused = [
        "v9.9.9-rc.1",
        "v9.9.9+git",
        "v09.9.9",
        "v0.08.0",
        "9.9.9",
        "v9.9",
        "v1000000000.0.0",
        "v0.0.1000000000",
    ];
    let temp = temp_repo();
    write_repo(&temp.repo, MANIFEST, LOCK, ONE, TWO);
    let before = snapshot(&temp.repo);
    for raw in refused {
        let message = parse_release_tag(raw).unwrap_err();
        assert!(message.contains(raw), "{message}");
        assert!(message.contains(TAG_PATTERN_IN_ERROR), "{message}");
        assert!(resolve(&temp.repo, "tag", raw).is_err());
        assert_eq!(snapshot(&temp.repo), before);
    }
    let parsed = parse_release_tag("v999999999.0.1").unwrap();
    assert_eq!(
        (parsed.major, parsed.minor, parsed.patch),
        (999999999, 0, 1)
    );
    assert_eq!(parsed.tag(), "v999999999.0.1");
    assert_eq!(parsed.cargo(), "999999999.0.1");
    assert_eq!(
        ReleaseVersion::try_new(10_i64.pow(9) - 1, 0, 0)
            .unwrap()
            .cargo(),
        "999999999.0.0"
    );
    for (major, minor, patch) in [(1_000_000_000, 0, 0), (0, -1, 0)] {
        assert!(ReleaseVersion::try_new(major, minor, patch).is_err());
    }
    assert_eq!(snapshot(&temp.repo), before);
}

fn test_plan_stamp_rewrites_only_member_path_versions() {
    let version = parse_release_tag("v9.9.9").unwrap();
    let (manifest, lock) = plan_stamp(MANIFEST, LOCK, &sample_members(), &version).unwrap();
    assert_eq!(manifest, EXPECTED_MANIFEST);
    assert_eq!(lock, EXPECTED_LOCK);
    assert!(lock.contains("\"some-crate 0.4.6\""));
    assert!(lock.contains("\nversion = 4\n"));
    // Python TypeError for plan_stamp(..., "v9.9.9") is not representable.
    assert!(
        plan_stamp(
            &format!("[package]\n{MANIFEST}"),
            LOCK,
            &sample_members(),
            &version
        )
        .is_err()
    );
}

fn test_plan_stamp_is_idempotent() {
    let version = parse_release_tag("v9.9.9").unwrap();
    let first = plan_stamp(MANIFEST, LOCK, &sample_members(), &version).unwrap();
    let second = plan_stamp(&first.0, &first.1, &sample_members(), &version).unwrap();
    assert_eq!(second, first);
    assert_eq!(
        first,
        (EXPECTED_MANIFEST.to_string(), EXPECTED_LOCK.to_string())
    );
}

fn test_branch_resolve_does_not_write() {
    let temp = temp_repo();
    write_repo(&temp.repo, MANIFEST, LOCK, ONE, TWO);
    let before = snapshot(&temp.repo);
    let version = resolve(&temp.repo, "branch", "123/merge").unwrap();
    assert_eq!(version.tag(), "v0.4.6");
    assert_eq!(version.cargo(), "0.4.6");
    assert_eq!(snapshot(&temp.repo), before);
}

fn test_tag_resolve_writes_then_second_call_skips() {
    let temp = temp_repo();
    write_repo(&temp.repo, MANIFEST, LOCK, ONE, TWO);
    let before = snapshot(&temp.repo);
    let env_path = temp.root.join("github.env");
    fs::write(&env_path, "").unwrap();
    let mut env = HashMap::new();
    env.insert("GITHUB_ENV".to_string(), env_path.display().to_string());
    let args = vec![
        "resolve".to_string(),
        "--repo".to_string(),
        temp.repo.display().to_string(),
        "--ref-type".to_string(),
        "tag".to_string(),
        "--ref-name".to_string(),
        "v9.9.9".to_string(),
    ];
    let stdout = dispatch(&args, &env).unwrap();
    assert_eq!(stdout, "v9.9.9\n");
    assert_eq!(fs::read_to_string(&env_path).unwrap(), "VERSION=v9.9.9\n");
    let after = snapshot(&temp.repo);
    assert_ne!(after[0].0, before[0].0);
    assert_ne!(after[1].0, before[1].0);
    assert_eq!(after[2].0, before[2].0);
    assert_eq!(after[3].0, before[3].0);
    assert_eq!(
        fs::read_to_string(temp.repo.join("Cargo.toml")).unwrap(),
        EXPECTED_MANIFEST
    );
    assert_eq!(
        fs::read_to_string(temp.repo.join("Cargo.lock")).unwrap(),
        EXPECTED_LOCK
    );
    let stamped = 1_000_000_000;
    for rel in ["Cargo.toml", "Cargo.lock"] {
        set_stamp(&temp.repo.join(rel), stamped);
    }
    resolve(&temp.repo, "tag", "v9.9.9").unwrap();
    assert_eq!(
        mtime_nanos(&temp.repo.join("Cargo.toml")),
        u128::from(stamped)
    );
    assert_eq!(
        mtime_nanos(&temp.repo.join("Cargo.lock")),
        u128::from(stamped)
    );
    assert_eq!(fs::read_to_string(&env_path).unwrap(), "VERSION=v9.9.9\n");
}

fn test_tag_resolve_stamps_crlf_manifest_and_lock() {
    let crlf = |text: &str| text.replace('\n', "\r\n").into_bytes();
    let temp = temp_repo();
    write_repo(&temp.repo, MANIFEST, LOCK, ONE, TWO);
    fs::write(temp.repo.join("Cargo.toml"), crlf(MANIFEST)).unwrap();
    fs::write(temp.repo.join("Cargo.lock"), crlf(LOCK)).unwrap();
    resolve(&temp.repo, "tag", "v9.9.9").unwrap();
    assert_eq!(
        fs::read(temp.repo.join("Cargo.toml")).unwrap(),
        crlf(EXPECTED_MANIFEST)
    );
    assert_eq!(
        fs::read(temp.repo.join("Cargo.lock")).unwrap(),
        crlf(EXPECTED_LOCK)
    );
}

fn test_missing_path_package_and_non_inherited_member_write_nothing() {
    let needle = "[[package]]\nname = \"one\"\nversion = \"0.4.6\"\ndependencies = [\n \"some-crate 0.4.6\",\n]\n\n";
    let missing = LOCK.replacen(needle, "", 1);
    let temp = temp_repo();
    write_repo(&temp.repo, MANIFEST, &missing, ONE, TWO);
    let before = snapshot(&temp.repo);
    let err = resolve(&temp.repo, "tag", "v9.9.9").unwrap_err();
    assert!(
        err.contains("Cargo.lock has no path package for one"),
        "{err}"
    );
    assert_eq!(snapshot(&temp.repo), before);

    let other = temp.root.join("owned");
    fs::create_dir(&other).unwrap();
    write_repo(
        &other,
        MANIFEST,
        LOCK,
        "[package]\nname = \"one\"\nversion = \"0.4.6\"\n",
        TWO,
    );
    let before = snapshot(&other);
    let err = resolve(&other, "tag", "v9.9.9").unwrap_err();
    assert!(err.contains("one"), "{err}");
    assert_eq!(snapshot(&other), before);
}

fn test_unknown_ref_type_fails() {
    let temp = temp_repo();
    write_repo(&temp.repo, MANIFEST, LOCK, ONE, TWO);
    let before = snapshot(&temp.repo);
    let err = resolve(&temp.repo, "pull_request", "v9.9.9").unwrap_err();
    assert!(err.contains("pull_request"), "{err}");
    assert!(err.contains("refusing ref_type"), "{err}");
    assert_eq!(snapshot(&temp.repo), before);
}

fn test_clap_names_match_installer() {
    let source = fs::read_to_string(crate::repo_root().join("deploy/install.sh")).unwrap();
    let function = extract_function(&source, "reported_name");
    assert!(
        CLAP_COMMANDS
            .iter()
            .all(|(stem, _)| *stem != "kiln-network")
    );
    assert!(!CLAP_COMMANDS.is_empty());
    for (stem, command) in CLAP_COMMANDS {
        let script = format!("{function}\nreported_name \"$1\"");
        let output = Command::new("bash")
            .args(["-c", &script, "bash", stem])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{command}\n")
        );
    }
    let version = parse_release_tag("v9.9.9").unwrap();
    assert!(clap_line(Path::new("kiln-network"), &version).is_err());
}

fn test_packaged_names_round_trip_through_check_archives() {
    let temp = temp_repo();
    let repo = temp.root.join("pack-repo");
    let bins = temp.root.join("bins");
    let image = temp.root.join("image");
    let dist = temp.root.join("dist");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&bins).unwrap();
    fs::create_dir(&image).unwrap();
    for name in ["kiln", "kiln.exe", "kiln-runtime", "kiln-host", "kiln-api"] {
        fs::write(bins.join(name), format!("binary-{name}")).unwrap();
    }
    write_named(&repo, PACKAGE_FILES);
    for name in ["image.json", "inputs.json", "rootfs.ext4", "vmlinux"] {
        fs::write(image.join(name), name).unwrap();
    }
    let version = parse_release_tag("v9.9.9").unwrap();
    crate::release::package(
        &repo,
        &bins,
        &dist,
        "v9.9.9",
        "x86_64-unknown-linux-gnu",
        "client",
        None,
    )
    .unwrap();
    crate::release::package(
        &repo,
        &bins,
        &dist,
        "v9.9.9",
        "x86_64-pc-windows-msvc",
        "client",
        None,
    )
    .unwrap();
    let server = crate::release::package(
        &repo,
        &bins,
        &dist,
        "v9.9.9",
        "x86_64-unknown-linux-gnu",
        "server",
        Some(&image),
    )
    .unwrap();
    check_archives(&version, &dist).unwrap();
    assert!(check_archives(&parse_release_tag("v9.9.8").unwrap(), &dist).is_err());
    rewrite_release_json(&server, "v9.9.8");
    let err = check_archives(&version, &dist).unwrap_err();
    assert!(err.contains("release.json"), "{err}");
    let longer = temp.root.join("longer");
    fs::create_dir(&longer).unwrap();
    let stray = "kiln-v9.9.90-x86_64-unknown-linux-gnu.tar.gz";
    fs::write(longer.join(stray), b"not-the-shorter-tag").unwrap();
    fs::write(
        longer.join(format!("{stray}.sha256")),
        format!("00  {stray}\n"),
    )
    .unwrap();
    assert!(check_archives(&version, &longer).is_err());
    let empty = temp.root.join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(check_archives(&version, &empty).is_err());
}

fn test_render_bootstrap_changes_only_the_version_and_digests() {
    let path = crate::repo_root().join("deploy/bootstrap/install.sh");
    let source = fs::read_to_string(&path).unwrap();
    let before = source.as_bytes().to_vec();
    let version = parse_release_tag("v9.9.9").unwrap();
    let digests = ASSET_KEYS
        .iter()
        .enumerate()
        .map(|(index, key)| {
            let byte = format!("{:02x}", index + 1);
            ((*key).to_string(), byte.repeat(32))
        })
        .collect::<HashMap<_, _>>();
    let rendered = render_bootstrap(&source, &version, &digests).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    let version_lines: Vec<_> = rendered
        .lines()
        .filter(|line| line.starts_with("    version="))
        .collect();
    assert_eq!(version_lines, ["    version=9.9.9"]);
    assert_eq!(asset_digests(&rendered), digests);
    let original_lines = split_lines_keep(&source);
    let rendered_lines = split_lines_keep(&rendered);
    assert_eq!(rendered_lines.len(), original_lines.len());
    let version_re = Regex::new(r"^    version=[0-9]+\.[0-9]+\.[0-9]+$").unwrap();
    let asset_re = Regex::new(r"^(\S+:\S+) ([0-9a-f]{64})$").unwrap();
    let changed: Vec<_> = original_lines
        .iter()
        .zip(rendered_lines.iter())
        .filter(|(left, right)| left != right)
        .collect();
    assert_eq!(changed.len(), 5);
    let mut saw_version = false;
    let mut seen = Vec::new();
    for (before_line, after_line) in changed {
        let (before_body, before_end) = split_ending(before_line);
        let (after_body, after_end) = split_ending(after_line);
        assert_eq!(before_end, after_end);
        if version_re.is_match(before_body) {
            assert_eq!(after_body, "    version=9.9.9");
            saw_version = true;
            continue;
        }
        let before_asset = asset_re.captures(before_body).unwrap();
        let after_asset = asset_re.captures(after_body).unwrap();
        assert_eq!(&before_asset[1], &after_asset[1]);
        let key: &str = &before_asset[1];
        assert_eq!(&after_asset[2], digests[key].as_str());
        seen.push(key.to_string());
    }
    assert!(saw_version);
    assert!(
        seen.iter()
            .map(String::as_str)
            .eq(ASSET_KEYS.iter().copied())
    );
}

fn test_render_bootstrap_rejects_a_missing_or_duplicate_version_line() {
    let source =
        fs::read_to_string(crate::repo_root().join("deploy/bootstrap/install.sh")).unwrap();
    let version = parse_release_tag("v9.9.9").unwrap();
    let digests = ASSET_KEYS
        .iter()
        .map(|key| ((*key).to_string(), "ab".repeat(32)))
        .collect::<HashMap<_, _>>();
    let pin_re = Regex::new(r"(?m)^(    version=[0-9]+\.[0-9]+\.[0-9]+\n)").unwrap();
    let pin_line = pin_re.captures(&source).unwrap().get(1).unwrap().as_str();
    let missing = source.replacen(pin_line, "", 1);
    let duplicate = source.replacen(pin_line, &format!("{pin_line}    version=1.2.3\n"), 1);
    for text in [missing, duplicate] {
        assert!(render_bootstrap(&text, &version, &digests).is_err());
    }
}

fn test_render_bootstrap_rejects_a_bad_digest_or_key_set() {
    let source =
        fs::read_to_string(crate::repo_root().join("deploy/bootstrap/install.sh")).unwrap();
    let version = parse_release_tag("v9.9.9").unwrap();
    let digests = ASSET_KEYS
        .iter()
        .map(|key| ((*key).to_string(), "ab".repeat(32)))
        .collect::<HashMap<_, _>>();
    let mut bad_digest = digests.clone();
    bad_digest.insert(ASSET_KEYS[0].to_string(), "A".repeat(64));
    let mut short = digests.clone();
    short.insert(ASSET_KEYS[0].to_string(), "abcd".to_string());
    let mut missing_key = digests.clone();
    missing_key.remove(ASSET_KEYS[ASSET_KEYS.len() - 1]);
    let mut extra_key = digests.clone();
    extra_key.insert("client:x86_64-pc-windows-msvc".to_string(), "cd".repeat(32));
    for mapping in [bad_digest, short, missing_key, extra_key] {
        assert!(render_bootstrap(&source, &version, &mapping).is_err());
    }
}

fn test_attach_bootstrap_rejects_a_missing_or_bad_sidecar() {
    let version = parse_release_tag("v9.9.9").unwrap();
    let repo_script = crate::repo_root().join("deploy/bootstrap/install.sh");
    let before = fs::read(&repo_script).unwrap();
    let temp = temp_repo();
    let missing = temp.root.join("missing-sidecars");
    fs::create_dir(&missing).unwrap();
    assert!(attach_bootstrap(&crate::repo_root(), &missing, &version).is_err());
    assert!(fs::read_dir(&missing).unwrap().next().is_none());

    let bad = temp.root.join("bad-sidecar");
    fs::create_dir(&bad).unwrap();
    let names: Vec<String> = ASSET_KEYS.iter().map(|key| sidecar_name(key)).collect();
    let good = "ab".repeat(32);
    for name in &names {
        let archive = name.strip_suffix(".sha256").unwrap();
        fs::write(bad.join(name), format!("{good}  {archive}\n")).unwrap();
    }
    let archive = names[0].strip_suffix(".sha256").unwrap();
    fs::write(bad.join(&names[0]), format!("not-a-digest  {archive}\n")).unwrap();
    let saved = dir_bytes(&bad);
    assert!(attach_bootstrap(&crate::repo_root(), &bad, &version).is_err());
    assert_eq!(dir_bytes(&bad), saved);
    assert!(!bad.join("install.sh").exists());
    assert_eq!(fs::read(&repo_script).unwrap(), before);
}

fn test_attach_bootstrap_cli_rejects_a_bad_tag_without_writing() {
    let temp = temp_repo();
    let dist = temp.root.join("cli-dist");
    fs::create_dir(&dist).unwrap();
    fs::write(dist.join("install.sh"), b"keep\n").unwrap();
    let before_dist = dir_bytes(&dist);
    let repo_script = crate::repo_root().join("deploy/bootstrap/install.sh");
    let before_repo = fs::read(&repo_script).unwrap();
    for raw in ["9.9.9", "v9.9.9-rc.1"] {
        let args = vec![
            "attach-bootstrap".to_string(),
            "--version".to_string(),
            raw.to_string(),
            "--repo".to_string(),
            crate::repo_root().display().to_string(),
            "--dist".to_string(),
            dist.display().to_string(),
        ];
        assert!(dispatch(&args, &HashMap::new()).is_err());
    }
    assert_eq!(dir_bytes(&dist), before_dist);
    assert_eq!(fs::read(&repo_script).unwrap(), before_repo);
}

fn test_attached_installer_runs_and_refuses_a_bad_checksum() {
    let temp = temp_repo();
    let repo = temp.root.join("pack-repo");
    let bins = temp.root.join("bins");
    let image = temp.root.join("image");
    let dist = temp.root.join("dist");
    fs::create_dir_all(&bins).unwrap();
    fs::create_dir_all(&image).unwrap();
    let client = bins.join("kiln");
    fs::write(&client, "#!/bin/sh\nprintf 'kiln 9.9.9\\n'\n").unwrap();
    set_mode(&client, 0o755).unwrap();
    for name in ["kiln-runtime", "kiln-host", "kiln-api"] {
        fs::write(bins.join(name), format!("stub-{name}")).unwrap();
    }
    write_named(&repo, PACKAGE_FILES);
    for name in ["image.json", "inputs.json", "rootfs.ext4", "vmlinux"] {
        fs::write(image.join(name), name).unwrap();
    }
    let version = parse_release_tag("v9.9.9").unwrap();
    for target in [
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ] {
        crate::release::package(&repo, &bins, &dist, "v9.9.9", target, "client", None).unwrap();
    }
    crate::release::package(
        &repo,
        &bins,
        &dist,
        "v9.9.9",
        "x86_64-unknown-linux-gnu",
        "server",
        Some(&image),
    )
    .unwrap();
    let repo_script = crate::repo_root().join("deploy/bootstrap/install.sh");
    let before_repo = fs::read(&repo_script).unwrap();
    assert!(!dist.join("install.sh").exists());
    check_archives(&version, &dist).unwrap();
    let archives = dir_bytes(&dist);
    let written = attach_bootstrap(&crate::repo_root(), &dist, &version).unwrap();
    assert_eq!(written, dist.join("install.sh"));
    let first_bytes = fs::read(&written).unwrap();
    let first_checksum = fs::read(dist.join("install.sh.sha256")).unwrap();
    let args = vec![
        "attach-bootstrap".to_string(),
        "--version".to_string(),
        "v9.9.9".to_string(),
        "--dist".to_string(),
        dist.display().to_string(),
    ];
    dispatch(&args, &HashMap::new()).unwrap();
    assert_eq!(fs::read(&written).unwrap(), first_bytes);
    assert_eq!(
        fs::read(dist.join("install.sh.sha256")).unwrap(),
        first_checksum
    );
    let after = dir_bytes(&dist);
    for (name, bytes) in &archives {
        assert_eq!(after.get(name).unwrap(), bytes);
    }
    assert_eq!(fs::read(&repo_script).unwrap(), before_repo);
    let script = fs::read_to_string(&written).unwrap();
    let version_lines: Vec<_> = script
        .lines()
        .filter(|line| line.starts_with("    version="))
        .collect();
    assert_eq!(version_lines, ["    version=9.9.9"]);
    let attached = asset_digests(&script);
    for key in ASSET_KEYS {
        let sidecar = fs::read_to_string(dist.join(sidecar_name(key))).unwrap();
        let digest = sidecar.split_whitespace().next().unwrap();
        assert_eq!(attached[*key].as_str(), digest);
    }
    let digest = sha256_hex(&fs::read(&written).unwrap());
    assert_eq!(
        fs::read_to_string(dist.join("install.sh.sha256")).unwrap(),
        format!("{digest}  install.sh\n")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&written).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
    let mut checksums: Vec<_> = fs::read_dir(&dist)
        .unwrap()
        .map(|entry| file_name(&entry.unwrap().path()))
        .filter(|name| name.ends_with(".sha256"))
        .collect();
    checksums.sort();
    let checked = Command::new("sha256sum")
        .arg("-c")
        .args(&checksums)
        .current_dir(&dist)
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );

    let home = temp.root.join("home");
    let tmp = temp.root.join("tmp");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&tmp).unwrap();
    let linux_archive = dist.join("kiln-v9.9.9-x86_64-unknown-linux-gnu.tar.gz");
    let served = temp.root.join("served.tar.gz");
    fs::copy(&linux_archive, &served).unwrap();
    let record = temp.root.join("urls");
    let stub = temp.root.join("stub-bin");
    fs::create_dir(&stub).unwrap();
    let python = python_executable();
    fs::write(
        stub.join("curl"),
        format!(
            "#!{python}\nimport os, sys\nfrom pathlib import Path\nargs = sys.argv[1:]\nurl = args[-1]\nPath(os.environ[\"CURL_RECORD\"]).open(\"a\", encoding=\"utf-8\").write(url + \"\\n\")\nif url != {LINUX_CLIENT_URL:?}:\n    sys.exit(1)\nif \"--output\" not in args or \"--write-out\" not in args:\n    sys.exit(2)\noutput = Path(args[args.index(\"--output\") + 1])\noutput.write_bytes(Path(os.environ[\"CURL_ARCHIVE\"]).read_bytes())\nsys.stdout.write(\"200\")\n"
        ),
    )
    .unwrap();
    set_mode(&stub.join("curl"), 0o755).unwrap();
    fs::write(
        stub.join("uname"),
        "#!/bin/sh\nif [ \"$1\" = -s ]; then printf \"Linux\\n\"; exit 0; fi\nif [ \"$1\" = -m ]; then printf \"x86_64\\n\"; exit 0; fi\nexit 2\n",
    )
    .unwrap();
    fs::write(
        stub.join("getconf"),
        "#!/bin/sh\n[ \"$1\" = GNU_LIBC_VERSION ] || exit 2\nprintf 'glibc 2.39\\n'\n",
    )
    .unwrap();
    set_mode(&stub.join("uname"), 0o755).unwrap();
    set_mode(&stub.join("getconf"), 0o755).unwrap();
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        format!("{}:/usr/bin:/bin", stub.display()),
    );
    env.insert("HOME".to_string(), home.display().to_string());
    env.insert("TMPDIR".to_string(), tmp.display().to_string());
    env.insert("CURL_ARCHIVE".to_string(), served.display().to_string());
    env.insert("CURL_RECORD".to_string(), record.display().to_string());
    env.insert("LANG".to_string(), "C".to_string());
    let installed = home.join(".local/bin/kiln");
    assert!(!installed.exists());
    let first = run_installer(&written, &env);
    assert!(
        first.status.success(),
        "{}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        fs::read_to_string(&record)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [LINUX_CLIENT_URL]
    );
    let version_run = Command::new(&installed).arg("--version").output().unwrap();
    assert!(version_run.status.success());
    assert_eq!(String::from_utf8_lossy(&version_run.stdout), "kiln 9.9.9\n");
    let before_install = fs::read(&installed).unwrap();
    let before_mtime = fs::metadata(&installed).unwrap().modified().unwrap();
    fs::write(&served, b"not-the-archive\n").unwrap();
    let second = run_installer(&written, &env);
    assert!(!second.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(combined.to_lowercase().contains("checksum"), "{combined}");
    assert_eq!(fs::read(&installed).unwrap(), before_install);
    assert_eq!(
        fs::metadata(&installed).unwrap().modified().unwrap(),
        before_mtime
    );
    assert_eq!(
        fs::read_to_string(&record)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [LINUX_CLIENT_URL, LINUX_CLIENT_URL]
    );
    assert_eq!(
        fs::read(&linux_archive).unwrap().as_slice(),
        archives[linux_archive.file_name().unwrap().to_str().unwrap()].as_slice()
    );
    assert_eq!(fs::read(&repo_script).unwrap(), before_repo);
    let server_archive = dist.join("kiln-server-v9.9.9-x86_64-unknown-linux-gnu.tar.gz");
    let (server_run, server_urls) = run_attached_server(&temp.root, &written, &server_archive);
    let server_output = format!(
        "{}{}",
        String::from_utf8_lossy(&server_run.stdout),
        String::from_utf8_lossy(&server_run.stderr)
    );
    assert!(
        !server_output.contains("Unexpected archive contents"),
        "{server_output}"
    );
    assert!(
        !server_output.to_lowercase().contains("checksum mismatch"),
        "{server_output}"
    );
    assert!(
        server_output.contains("Fresh Kiln installation"),
        "{server_output}"
    );
    assert_eq!(server_urls, vec![LINUX_SERVER_URL.to_string()]);
    assert_eq!(
        fs::read(&server_archive).unwrap().as_slice(),
        archives[server_archive.file_name().unwrap().to_str().unwrap()].as_slice()
    );
    assert_eq!(fs::read(&repo_script).unwrap(), before_repo);
}

fn extract_function(source: &str, name: &str) -> String {
    let marker = format!("{name}() {{");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {name}"));
    let bytes = source.as_bytes();
    let mut depth = 0i32;
    for index in start..bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=index].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unclosed {name}");
}

fn split_ending(line: &str) -> (&str, &str) {
    if let Some(text) = line.strip_suffix("\r\n") {
        (text, "\r\n")
    } else if let Some(text) = line.strip_suffix('\n') {
        (text, "\n")
    } else {
        (line, "")
    }
}

fn asset_digests(text: &str) -> HashMap<String, String> {
    let lines = split_lines(text);
    let begin = lines.iter().position(|line| *line == ASSET_BEGIN).unwrap();
    let end = lines.iter().position(|line| *line == ASSET_END).unwrap();
    let mut digests = HashMap::new();
    for line in &lines[begin + 1..end] {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap().to_string();
        let digest = parts.next().unwrap().to_string();
        digests.insert(key, digest);
    }
    digests
}

fn sidecar_name(key: &str) -> String {
    let (mode, target) = key.split_once(':').unwrap();
    let prefix = match mode {
        "client" => "kiln",
        "server" => "kiln-server",
        _ => panic!("unknown asset {key}"),
    };
    format!("{prefix}-v9.9.9-{target}.tar.gz.sha256")
}

fn run_attached_server(root: &Path, script: &Path, archive: &Path) -> (Captured, Vec<String>) {
    let fixture = root.join("server-fixture");
    fs::create_dir(&fixture).unwrap();
    fs::create_dir(fixture.join("kvm")).unwrap();
    fs::create_dir(fixture.join("systemd")).unwrap();
    fs::write(
        fixture.join("os-release"),
        "ID=ubuntu\nVERSION_ID=\"24.04\"\n",
    )
    .unwrap();
    fs::write(fixture.join("controllers"), "cpu memory pids\n").unwrap();
    fs::write(fixture.join("meminfo"), "MemAvailable:   8388608 kB\n").unwrap();
    let mut text = fs::read_to_string(script).unwrap();
    for (old, new) in [
        ("/etc/os-release", fixture.join("os-release")),
        ("/dev/kvm", fixture.join("kvm")),
        ("/proc/meminfo", fixture.join("meminfo")),
        ("/run/systemd/system", fixture.join("systemd")),
        (
            "/sys/fs/cgroup/cgroup.controllers",
            fixture.join("controllers"),
        ),
        ("/etc/kiln", fixture.join("etc-kiln")),
        ("/opt/kiln", fixture.join("opt-kiln")),
        ("/var/lib/kiln", fixture.join("state-kiln")),
    ] {
        text = text.replace(old, &new.display().to_string());
    }
    let rewritten = root.join("server-install.sh");
    fs::write(&rewritten, text).unwrap();
    set_mode(&rewritten, 0o755).unwrap();
    let stub = root.join("server-stub");
    fs::create_dir(&stub).unwrap();
    let record = root.join("server-urls");
    let python = python_executable();
    fs::write(
        stub.join("curl"),
        format!(
            "#!{python}\nimport os, sys\nfrom pathlib import Path\nargs = sys.argv[1:]\nurl = args[-1]\nPath(os.environ[\"CURL_RECORD\"]).open(\"a\", encoding=\"utf-8\").write(url + \"\\n\")\nif url != {LINUX_SERVER_URL:?}:\n    sys.exit(1)\noutput = Path(args[args.index(\"--output\") + 1])\noutput.write_bytes(Path(os.environ[\"CURL_ARCHIVE\"]).read_bytes())\nsys.stdout.write(\"200\")\n"
        ),
    )
    .unwrap();
    fs::write(
        stub.join("sudo"),
        format!(
            "#!{python}\nimport subprocess, sys\nargs = sys.argv[1:]\nif args[:1] == ['-n']:\n    args = args[1:]\nif args == ['true']:\n    raise SystemExit(0)\nif args[:1] == ['sh'] and any('kvm' in part for part in args):\n    raise SystemExit(0)\nif 'apt-get' in args:\n    raise SystemExit(0)\nif args[:1] == ['bash']:\n    raise SystemExit(subprocess.call(args))\nraise SystemExit(3)\n"
        ),
    )
    .unwrap();
    fs::write(stub.join("id"), "#!/bin/sh\nprintf '1000\\n'\n").unwrap();
    fs::write(
        stub.join("uname"),
        "#!/bin/sh\nif [ \"$1\" = -s ]; then printf \"Linux\\n\"; exit 0; fi\nif [ \"$1\" = -m ]; then printf \"x86_64\\n\"; exit 0; fi\nexit 2\n",
    )
    .unwrap();
    fs::write(
        stub.join("getconf"),
        "#!/bin/sh\n[ \"$1\" = GNU_LIBC_VERSION ] || exit 2\nprintf 'glibc 2.39\\n'\n",
    )
    .unwrap();
    fs::write(
        stub.join("ip"),
        "#!/bin/sh\n[ \"$*\" = \"-4 route get 1.1.1.1\" ] || exit 2\nprintf '%s\\n' '1.1.1.1 via 192.168.50.1 dev eth0 src 192.168.50.7'\n",
    )
    .unwrap();
    fs::write(
        stub.join("df"),
        "#!/bin/sh\n[ \"$1\" = -B1 ] || exit 2\nprintf '%s\\n' 'Filesystem 1B-blocks Used Available Use% Mounted' 'fixture 30000000000 1 30000000000 1% /'\n",
    )
    .unwrap();
    for entry in fs::read_dir(&stub).unwrap() {
        set_mode(&entry.unwrap().path(), 0o755).unwrap();
    }
    let home = root.join("server-home");
    let tmp = root.join("server-tmp");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&tmp).unwrap();
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        format!("{}:/usr/bin:/bin", stub.display()),
    );
    env.insert("HOME".to_string(), home.display().to_string());
    env.insert("TMPDIR".to_string(), tmp.display().to_string());
    env.insert("CURL_ARCHIVE".to_string(), archive.display().to_string());
    env.insert("CURL_RECORD".to_string(), record.display().to_string());
    env.insert("LANG".to_string(), "C".to_string());
    let mut command = Command::new("sh");
    command.arg(&rewritten).arg("server");
    command.env_clear().envs(&env);
    detach(&mut command);
    let completed = capture(command, None, Duration::from_secs(30)).expect("server installer");
    let urls = if record.exists() {
        fs::read_to_string(&record)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    (completed, urls)
}

fn run_installer(script: &Path, env: &HashMap<String, String>) -> Captured {
    let mut command = Command::new("sh");
    command.env_clear().envs(env);
    detach(&mut command);
    let input = fs::read(script).unwrap();
    capture(command, Some(&input), Duration::from_secs(30)).expect("installer")
}

fn python_executable() -> String {
    let output = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn rewrite_release_json(archive: &Path, version: &str) {
    let file = File::open(archive).unwrap();
    let decoder = GzDecoder::new(file);
    let mut src = tar::Archive::new(decoder);
    let mut members = Vec::new();
    for entry in src.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        let mode = entry.header().mode().unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        members.push((name, mode, data));
    }
    drop(src);
    for (name, _, data) in &mut members {
        if name == "release.json" {
            let mut payload: serde_json::Value = serde_json::from_slice(data).unwrap();
            payload["version"] = serde_json::Value::String(version.to_string());
            *data = serde_json::to_vec(&payload).unwrap();
        }
    }
    let file = File::create(archive).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut dst = tar::Builder::new(encoder);
    for (name, mode, data) in &members {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_mtime(0);
        header.set_cksum();
        dst.append(&header, data.as_slice()).unwrap();
    }
    let encoder = dst.into_inner().unwrap();
    encoder.finish().unwrap();
}

register_cases! {
    test_rejects_prerelease_build_metadata_leading_zeros_and_bare_cargo,
    test_plan_stamp_rewrites_only_member_path_versions,
    test_plan_stamp_is_idempotent,
    test_branch_resolve_does_not_write,
    test_tag_resolve_writes_then_second_call_skips,
    test_tag_resolve_stamps_crlf_manifest_and_lock,
    test_missing_path_package_and_non_inherited_member_write_nothing,
    test_unknown_ref_type_fails,
    test_clap_names_match_installer,
    test_packaged_names_round_trip_through_check_archives,
    test_render_bootstrap_changes_only_the_version_and_digests,
    test_render_bootstrap_rejects_a_missing_or_duplicate_version_line,
    test_render_bootstrap_rejects_a_bad_digest_or_key_set,
    test_attach_bootstrap_rejects_a_missing_or_bad_sidecar,
    test_attach_bootstrap_cli_rejects_a_bad_tag_without_writing,
    test_attached_installer_runs_and_refuses_a_bad_checksum,
}
