use crate::register_cases;
use crate::release_version::parse_release_tag;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

const KINDS: &[&str] = &["client", "server", "identity", "dashboard"];

#[derive(Debug)]
pub enum PackageError {
    Invalid(String),
    Missing(String),
    Exists(String),
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Missing(message) | Self::Exists(message) => {
                formatter.write_str(message)
            }
        }
    }
}

pub fn package(
    repo: &Path,
    bins: &Path,
    output: &Path,
    version: &str,
    target: &str,
    kind: &str,
    image: Option<&Path>,
) -> Result<PathBuf, PackageError> {
    let version = parse_release_tag(version)
        .map_err(PackageError::Invalid)?
        .tag();
    let allowed: &[&str] = if kind == "dashboard" {
        &["web"]
    } else {
        TARGETS
    };
    if !allowed.contains(&target) || !KINDS.contains(&kind) {
        return Err(PackageError::Invalid(
            "unsupported target or package kind".into(),
        ));
    }
    let windows = target.ends_with("windows-msvc");
    let (files, name) = match kind {
        "client" => client_files(&version, target, bins, windows)?,
        "dashboard" => dashboard_files(&version, bins)?,
        "identity" => identity_files(&version, target, repo, bins)?,
        "server" => server_files(&version, target, repo, bins, image)?,
        _ => unreachable!(),
    };
    for (_, path) in &files {
        if !is_regular_file(path) {
            return Err(PackageError::Missing(format!(
                "missing regular package input: {}",
                path.display()
            )));
        }
    }
    fs::create_dir_all(output).map_err(io_invalid)?;
    let dest = output.join(format!(
        "{name}{}",
        if windows { ".zip" } else { ".tar.gz" }
    ));
    let checksum = PathBuf::from(format!("{}.sha256", dest.display()));
    if dest.exists() || checksum.exists() {
        return Err(PackageError::Exists(format!(
            "refusing to overwrite {}",
            dest.display()
        )));
    }
    let archive = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .map_err(io_invalid)?;
    if windows {
        write_zip(archive, &files)?;
    } else {
        write_tar(archive, &files, kind, &version, target)?;
    }
    let digest = sha256_file(&dest)?;
    let mut sidecar = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&checksum)
        .map_err(io_invalid)?;
    sidecar
        .write_all(
            format!(
                "{digest}  {}\n",
                dest.file_name().unwrap().to_string_lossy()
            )
            .as_bytes(),
        )
        .map_err(io_invalid)?;
    Ok(dest)
}

fn client_files(
    version: &str,
    target: &str,
    bins: &Path,
    windows: bool,
) -> Result<(Vec<(String, PathBuf)>, String), PackageError> {
    let binary = if windows { "kiln.exe" } else { "kiln" };
    Ok((
        vec![(binary.to_string(), bins.join(binary))],
        format!("kiln-{version}-{target}"),
    ))
}

fn dashboard_files(
    version: &str,
    bins: &Path,
) -> Result<(Vec<(String, PathBuf)>, String), PackageError> {
    let assets = bins.join("assets");
    if is_symlink(bins) || is_symlink(&assets) || !is_real_dir(&assets) {
        return Err(PackageError::Invalid(
            "dashboard requires a regular static assets directory".into(),
        ));
    }
    let mut files = vec![
        ("index.html".to_string(), bins.join("index.html")),
        ("favicon.svg".to_string(), bins.join("favicon.svg")),
    ];
    let mut names = fs::read_dir(&assets)
        .map_err(io_invalid)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_invalid))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    for path in names {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let allowed = [".js", ".css", ".woff", ".woff2", ".svg", ".webp"]
            .iter()
            .any(|suffix| name.ends_with(suffix));
        if is_symlink(&path) || !is_regular_file(&path) || !allowed {
            return Err(PackageError::Invalid(format!(
                "unexpected dashboard asset: {name}"
            )));
        }
        files.push((format!("assets/{name}"), path));
    }
    Ok((files, format!("kiln-dashboard-{version}")))
}

fn identity_files(
    version: &str,
    target: &str,
    repo: &Path,
    bins: &Path,
) -> Result<(Vec<(String, PathBuf)>, String), PackageError> {
    if target != "x86_64-unknown-linux-gnu" {
        return Err(PackageError::Invalid(
            "identity requires Linux x86-64".into(),
        ));
    }
    let files = vec![
        ("bin/kiln-identity".into(), bins.join("kiln-identity")),
        (
            "deploy/kiln-identity.service".into(),
            repo.join("deploy/kiln-identity.service"),
        ),
        (
            "deploy/identity.example.env".into(),
            repo.join("deploy/identity.example.env"),
        ),
        (
            "docs/identity-service.md".into(),
            repo.join("docs/identity-service.md"),
        ),
    ];
    Ok((files, format!("kiln-identity-{version}-{target}")))
}

fn server_files(
    version: &str,
    target: &str,
    repo: &Path,
    bins: &Path,
    image: Option<&Path>,
) -> Result<(Vec<(String, PathBuf)>, String), PackageError> {
    let Some(image) = image else {
        return Err(PackageError::Invalid(
            "server requires Linux x86-64 and a prebuilt image directory".into(),
        ));
    };
    if target != "x86_64-unknown-linux-gnu" {
        return Err(PackageError::Invalid(
            "server requires Linux x86-64 and a prebuilt image directory".into(),
        ));
    }
    let mut files = Vec::new();
    for name in ["kiln-runtime", "kiln-host", "kiln-api", "kiln"] {
        files.push((format!("bin/{name}"), bins.join(name)));
    }
    files.push(("install.sh".into(), repo.join("deploy/install.sh")));
    files.push(("bin/kiln-network".into(), repo.join("deploy/kiln-network")));
    files.push((
        "fetch-firecracker.sh".into(),
        repo.join("scripts/fetch-firecracker.sh"),
    ));
    for name in [
        "README.md",
        "docs/releases.md",
        "docs/remote-client.md",
        "docs/identity-service.md",
        "docs/runtime.md",
        "docs/runbooks/2026-10-19-public-door.md",
        "docs/runbooks/2026-10-19-door-public-cert.md",
        "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
    ] {
        files.push((name.into(), repo.join(name)));
    }
    for name in [
        "kiln-host.service",
        "kiln-api.service",
        "kiln-door.service",
        "kiln-app.service",
        "kiln-app-proxy.service",
        "kiln-app-proxy.socket",
        "kiln-door-proxy.service",
        "kiln-door-proxy.socket",
        "paseo-caddy.override.yaml",
        "door.Caddyfile",
        "app.Caddyfile",
        "enroll-dark-forge.sh",
        "host.example.json",
    ] {
        files.push((format!("deploy/{name}"), repo.join("deploy").join(name)));
    }
    for name in ["image.json", "inputs.json", "vmlinux", "rootfs.ext4"] {
        files.push((format!("image/{name}"), image.join(name)));
    }
    Ok((files, format!("kiln-server-{version}-{target}")))
}

fn write_zip(file: File, files: &[(String, PathBuf)]) -> Result<(), PackageError> {
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (relative, path) in files {
        archive.start_file(relative, options).map_err(io_invalid)?;
        let mut source = File::open(path).map_err(io_invalid)?;
        io::copy(&mut source, &mut archive).map_err(io_invalid)?;
    }
    archive.finish().map_err(io_invalid)?;
    Ok(())
}

fn write_tar(
    file: File,
    files: &[(String, PathBuf)],
    kind: &str,
    version: &str,
    target: &str,
) -> Result<(), PackageError> {
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (relative, path) in files {
        append_file(&mut archive, relative, path)?;
    }
    if kind == "server" || kind == "identity" {
        let mut payload = serde_json::json!({
            "version": version,
            "target": target,
        });
        if kind == "server" {
            payload["min_upgrade_from"] = serde_json::json!("0.3.0");
        }
        let data = serde_json::to_vec(&payload).map_err(io_invalid)?;
        append_bytes(&mut archive, "release.json", &data, 0o644)?;
    }
    let encoder = archive.into_inner().map_err(io_invalid)?;
    encoder.finish().map_err(io_invalid)?;
    Ok(())
}

fn append_file<W: Write>(
    archive: &mut tar::Builder<W>,
    relative: &str,
    path: &Path,
) -> Result<(), PackageError> {
    let file = File::open(path).map_err(io_invalid)?;
    let size = file.metadata().map_err(io_invalid)?.len();
    append_reader(archive, relative, file, size, member_mode(relative))
}

fn append_bytes<W: Write>(
    archive: &mut tar::Builder<W>,
    relative: &str,
    data: &[u8],
    mode: u32,
) -> Result<(), PackageError> {
    append_reader(
        archive,
        relative,
        io::Cursor::new(data),
        data.len() as u64,
        mode,
    )
}

fn append_reader<W: Write, R: io::Read>(
    archive: &mut tar::Builder<W>,
    relative: &str,
    reader: R,
    size: u64,
    mode: u32,
) -> Result<(), PackageError> {
    let mut header = tar::Header::new_gnu();
    header.set_path(relative).map_err(io_invalid)?;
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("").map_err(io_invalid)?;
    header.set_groupname("").map_err(io_invalid)?;
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    archive.append(&header, reader).map_err(io_invalid)?;
    Ok(())
}

fn member_mode(relative: &str) -> u32 {
    if relative == "kiln" || relative == "install.sh" || relative.starts_with("bin/") {
        0o755
    } else {
        0o644
    }
}

fn sha256_file(path: &Path) -> Result<String, PackageError> {
    let mut file = File::open(path).map_err(io_invalid)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(io_invalid)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_file())
        .unwrap_or(false)
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_dir())
        .unwrap_or(false)
}

fn io_invalid<E: std::fmt::Display>(error: E) -> PackageError {
    PackageError::Invalid(error.to_string())
}

pub fn cli(args: &[String]) -> i32 {
    match cli_result(args) {
        Ok(path) => {
            println!("{}", path.display());
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn cli_result(args: &[String]) -> Result<PathBuf, PackageError> {
    let mut kind = None;
    let mut version = None;
    let mut target = None;
    let mut bin_dir = None;
    let mut image_dir = None;
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if kind.is_none() && !arg.starts_with('-') {
            kind = Some(arg.clone());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| PackageError::Invalid(format!("missing value for {arg}")))?;
        match arg.as_str() {
            "--version" => version = Some(value.clone()),
            "--target" => target = Some(value.clone()),
            "--bin-dir" => bin_dir = Some(PathBuf::from(value)),
            "--image-dir" => image_dir = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--" => {
                index += 1;
                continue;
            }
            other => {
                return Err(PackageError::Invalid(format!("unknown argument {other}")));
            }
        }
        index += 2;
    }
    package(
        &crate::repo_root(),
        bin_dir
            .as_deref()
            .ok_or_else(|| PackageError::Invalid("--bin-dir is required".into()))?,
        output
            .as_deref()
            .ok_or_else(|| PackageError::Invalid("--output is required".into()))?,
        version
            .as_deref()
            .ok_or_else(|| PackageError::Invalid("--version is required".into()))?,
        target
            .as_deref()
            .ok_or_else(|| PackageError::Invalid("--target is required".into()))?,
        kind.as_deref()
            .ok_or_else(|| PackageError::Invalid("package kind is required".into()))?,
        image_dir.as_deref(),
    )
}

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    bins: PathBuf,
    out: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let repo = root.join("repo");
    let bins = root.join("bin");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&bins).unwrap();
    for name in ["kiln", "kiln.exe", "kiln-runtime", "kiln-host", "kiln-api"] {
        fs::write(bins.join(name), format!("binary-{name}")).unwrap();
    }
    Fixture {
        repo,
        bins,
        out: root.join("dist"),
        _temp: temp,
    }
}

impl Fixture {
    fn package(
        &self,
        kind: &str,
        target: &str,
        image: Option<&Path>,
    ) -> Result<PathBuf, PackageError> {
        package(
            &self.repo, &self.bins, &self.out, "v0.1.0", target, kind, image,
        )
    }
}

fn test_client_allowlist_permissions_checksum_and_no_overwrite() {
    let fix = fixture();
    fs::write(fix.bins.join("admin.token"), "secret").unwrap();
    let path = fix
        .package("client", "x86_64-unknown-linux-gnu", None)
        .unwrap();
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        "kiln-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(tar_names(&path), vec!["kiln".to_string()]);
    assert_eq!(tar_mode(&path, "kiln") & 0o777, 0o755);
    assert_eq!(tar_bytes(&path, "kiln"), b"binary-kiln");
    let expected = sha256_file(&path).unwrap();
    let sidecar = fs::read_to_string(format!("{}.sha256", path.display())).unwrap();
    assert_eq!(
        sidecar,
        format!(
            "{expected}  {}\n",
            path.file_name().unwrap().to_string_lossy()
        )
    );
    assert!(matches!(
        fix.package("client", "x86_64-unknown-linux-gnu", None),
        Err(PackageError::Exists(_))
    ));
}

fn test_windows_zip_contains_only_exe() {
    let fix = fixture();
    let path = fix
        .package("client", "x86_64-pc-windows-msvc", None)
        .unwrap();
    let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive.by_index(0).unwrap().name(), "kiln.exe");
    let mut stored = Vec::new();
    io::copy(&mut archive.by_index(0).unwrap(), &mut stored).unwrap();
    assert_eq!(stored, b"binary-kiln.exe");
}

fn test_dashboard_contains_only_static_assets() {
    let fix = fixture();
    fs::create_dir(fix.repo.join("web")).unwrap();
    fs::write(
        fix.repo.join("web/README.md"),
        "Frontend only; no live control plane",
    )
    .unwrap();
    fs::create_dir(fix.bins.join("assets")).unwrap();
    for name in [
        "index.html",
        "favicon.svg",
        "THIRD_PARTY_LICENSES.md",
        "assets/index-ab.js",
        "assets/index-cd.css",
        "assets/font-ef.woff2",
        "assets/hero-gh.webp",
    ] {
        if let Some(parent) = Path::new(name).parent()
            && parent != Path::new("")
        {
            fs::create_dir_all(fix.bins.join(parent)).unwrap();
        }
        fs::write(fix.bins.join(name), name).unwrap();
    }
    fs::write(fix.bins.join("admin.token"), "private").unwrap();
    let path = fix.package("dashboard", "web", None).unwrap();
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        "kiln-dashboard-v0.1.0.tar.gz"
    );
    let names = tar_names(&path);
    assert_eq!(
        names
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "index.html",
            "favicon.svg",
            "assets/index-ab.js",
            "assets/index-cd.css",
            "assets/font-ef.woff2",
            "assets/hero-gh.webp",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
    let script = tar_bytes(&path, "assets/index-ab.js");
    assert_eq!(script, b"assets/index-ab.js");
    assert!(tar_modes(&path).iter().all(|mode| mode & 0o777 == 0o644));
    let sidecar = fs::read_to_string(format!("{}.sha256", path.display())).unwrap();
    assert_eq!(
        sidecar.split_whitespace().next().unwrap(),
        sha256_file(&path).unwrap()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            fix.bins.join("admin.token"),
            fix.bins.join("assets/private.js"),
        )
        .unwrap();
        assert!(matches!(
            fix.package("dashboard", "web", None),
            Err(PackageError::Invalid(_))
        ));
        fs::remove_file(fix.bins.join("assets/private.js")).unwrap();
    }
    fs::write(fix.bins.join("assets/private.pem"), "private").unwrap();
    assert!(matches!(
        fix.package("dashboard", "web", None),
        Err(PackageError::Invalid(_))
    ));
    assert!(matches!(
        fix.package("client", "web", None),
        Err(PackageError::Invalid(_))
    ));
}

fn test_identity_allowlist_excludes_vm_and_private_state() {
    let fix = fixture();
    fs::write(fix.bins.join("kiln-identity"), b"identity-binary").unwrap();
    fs::write(fix.bins.join("signing.pem"), "private fixture").unwrap();
    for name in [
        "deploy/kiln-identity.service",
        "deploy/identity.example.env",
        "docs/identity-service.md",
    ] {
        let path = fix.repo.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, name).unwrap();
    }
    let path = fix
        .package("identity", "x86_64-unknown-linux-gnu", None)
        .unwrap();
    let names = tar_names(&path);
    assert_eq!(
        names
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "bin/kiln-identity",
            "deploy/kiln-identity.service",
            "deploy/identity.example.env",
            "docs/identity-service.md",
            "release.json",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
    assert_eq!(tar_mode(&path, "bin/kiln-identity") & 0o777, 0o755);
    let release: serde_json::Value =
        serde_json::from_slice(&tar_bytes(&path, "release.json")).unwrap();
    assert_eq!(release["version"], "v0.1.0");
    assert!(release.get("min_upgrade_from").is_none());
    assert!(matches!(
        fix.package("identity", "x86_64-apple-darwin", None),
        Err(PackageError::Invalid(_))
    ));
}

fn test_invalid_version_target_and_missing_binary_make_no_archive() {
    let fix = fixture();
    for (version, target) in [
        ("../escape", "x86_64-unknown-linux-gnu"),
        ("v0.1.0-rc.1", "x86_64-unknown-linux-gnu"),
        ("v0.1.0", "arm-unknown"),
    ] {
        assert!(
            matches!(
                package(
                    &fix.repo, &fix.bins, &fix.out, version, target, "client", None
                ),
                Err(PackageError::Invalid(_))
            ),
            "{version}"
        );
        assert!(!fix.out.exists(), "{version}");
    }
    fs::remove_file(fix.bins.join("kiln")).unwrap();
    assert!(matches!(
        fix.package("client", "x86_64-unknown-linux-gnu", None),
        Err(PackageError::Missing(_))
    ));
    assert!(!fix.out.exists());
}

fn test_server_contains_prebuilt_image_not_host_snapshots_or_secrets() {
    let fix = fixture();
    for name in [
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
        "docs/runbooks/2026-10-19-door-public-cert.md",
        "deploy/host.example.json",
        "scripts/fetch-firecracker.sh",
        "README.md",
        "docs/releases.md",
        "docs/remote-client.md",
        "docs/identity-service.md",
        "docs/runtime.md",
        "docs/runbooks/2026-10-19-public-door.md",
        "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
    ] {
        let path = fix.repo.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, name).unwrap();
    }
    let image = fix._temp.path().join("image");
    fs::create_dir(&image).unwrap();
    for name in ["image.json", "inputs.json", "rootfs.ext4", "vmlinux"] {
        fs::write(image.join(name), name).unwrap();
    }
    fs::write(image.join("snapshot.memory"), "private VM memory").unwrap();
    fs::write(image.join("admin.token"), "secret").unwrap();
    let path = fix
        .package("server", "x86_64-unknown-linux-gnu", Some(&image))
        .unwrap();
    let names: std::collections::BTreeSet<_> = tar_names(&path).into_iter().collect();
    let expected = [
        "bin/kiln-runtime",
        "bin/kiln-host",
        "bin/kiln-api",
        "bin/kiln",
        "bin/kiln-network",
        "install.sh",
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
        "fetch-firecracker.sh",
        "README.md",
        "docs/releases.md",
        "docs/remote-client.md",
        "docs/identity-service.md",
        "docs/runtime.md",
        "docs/runbooks/2026-10-19-public-door.md",
        "docs/runbooks/2026-10-19-door-public-cert.md",
        "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
        "image/image.json",
        "image/inputs.json",
        "image/rootfs.ext4",
        "image/vmlinux",
        "release.json",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names, expected);
    let release: serde_json::Value =
        serde_json::from_slice(&tar_bytes(&path, "release.json")).unwrap();
    assert_eq!(release["version"], "v0.1.0");
    assert_eq!(release["target"], "x86_64-unknown-linux-gnu");
    assert_eq!(release["min_upgrade_from"], "0.3.0");
    assert_eq!(tar_mode(&path, "install.sh") & 0o777, 0o755);
    assert_eq!(names, bootstrap_accepts(&names));
    let app_units = [
        "deploy/kiln-app.service",
        "deploy/kiln-app-proxy.service",
        "deploy/kiln-app-proxy.socket",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<std::collections::BTreeSet<_>>();
    assert!(app_units.is_subset(&names));
    let mut partial = names.clone();
    partial.remove("deploy/kiln-app-proxy.socket");
    assert_eq!(bootstrap_accepts(&partial), names);
    assert_ne!(partial, bootstrap_accepts(&partial));
    let mut without_app = names.clone();
    for name in &app_units {
        without_app.remove(name);
    }
    assert_eq!(bootstrap_accepts(&without_app), without_app);
    let app_site = [
        "deploy/app.Caddyfile",
        "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<std::collections::BTreeSet<_>>();
    assert!(app_site.is_subset(&names));
    let mut site_half = names.clone();
    site_half.remove("deploy/app.Caddyfile");
    assert_eq!(bootstrap_accepts(&site_half), names);
    assert_ne!(site_half, bootstrap_accepts(&site_half));
    let mut without_site = names.clone();
    for name in &app_site {
        without_site.remove(name);
    }
    assert_eq!(bootstrap_accepts(&without_site), without_site);
    let door = fs::read_to_string(crate::repo_root().join("deploy/kiln-door.service")).unwrap();
    assert!(!door.contains("dashboard-root"));
    assert!(
        names
            .iter()
            .all(|name| !name.starts_with("kiln-dashboard-"))
    );
}

fn test_bootstrap_accepts_windows_crlf_checkout() {
    let on_disk =
        fs::read_to_string(crate::repo_root().join("deploy/bootstrap/install.sh")).unwrap();
    let lf = on_disk.replace('\r', "");
    assert!(lf.contains('\n'));
    let archive = std::collections::BTreeSet::new();
    let expected = bootstrap_accepts_text(&lf, &archive);
    let crlf = lf.replace('\n', "\r\n");
    assert!(crlf.contains("\r\n"));
    assert_eq!(bootstrap_accepts_text(&crlf, &archive), expected);
    assert_eq!(bootstrap_accepts_text(&on_disk, &archive), expected);
    assert!(expected.iter().all(|name| !name.contains('\r')));
}

fn bootstrap_accepts(
    archive_names: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let text = fs::read_to_string(crate::repo_root().join("deploy/bootstrap/install.sh")).unwrap();
    bootstrap_accepts_text(&text, archive_names)
}

fn bootstrap_accepts_text(
    text: &str,
    archive_names: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let text = text.replace('\r', "");
    let marker = "cat > \"$work/expected\" <<FILES\n";
    let start = text.find(marker).unwrap() + marker.len();
    let end = text[start..].find("\nFILES\n").unwrap();
    let mut expected: std::collections::BTreeSet<String> = text[start..start + end]
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect();
    if archive_names.contains("bin/kiln-network") {
        expected.insert("bin/kiln-network".into());
    }
    let door_core = [
        "deploy/kiln-door-proxy.service",
        "deploy/kiln-door-proxy.socket",
        "docs/runbooks/2026-10-19-door-public-cert.md",
    ];
    let public = "deploy/kiln-door-proxy-public.socket";
    let caddy = ["deploy/paseo-caddy.override.yaml", "deploy/door.Caddyfile"];
    if archive_names
        .iter()
        .any(|name| door_core.contains(&name.as_str()) || name == public)
    {
        expected.extend(door_core.into_iter().map(str::to_string));
    }
    if archive_names.contains(public) {
        expected.insert(public.into());
    }
    if archive_names
        .iter()
        .any(|name| caddy.contains(&name.as_str()))
    {
        expected.extend(caddy.into_iter().map(str::to_string));
    }
    let app_units = [
        "deploy/kiln-app.service",
        "deploy/kiln-app-proxy.service",
        "deploy/kiln-app-proxy.socket",
    ];
    if archive_names
        .iter()
        .any(|name| app_units.contains(&name.as_str()))
    {
        expected.extend(app_units.into_iter().map(str::to_string));
    }
    let app_site = [
        "deploy/app.Caddyfile",
        "docs/runbooks/2026-10-20-app-dark-forge-caddy.md",
    ];
    if archive_names
        .iter()
        .any(|name| app_site.contains(&name.as_str()))
    {
        expected.extend(app_site.into_iter().map(str::to_string));
    }
    expected
}

fn tar_names(path: &Path) -> Vec<String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(path).unwrap()));
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
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(path).unwrap()));
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().to_string_lossy() == name {
            let mut stored = Vec::new();
            io::copy(&mut entry, &mut stored).unwrap();
            return stored;
        }
    }
    panic!("missing {name}");
}

fn tar_mode(path: &Path, name: &str) -> u32 {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(path).unwrap()));
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        if entry.path().unwrap().to_string_lossy() == name {
            return entry.header().mode().unwrap();
        }
    }
    panic!("missing {name}");
}

fn tar_modes(path: &Path) -> Vec<u32> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(path).unwrap()));
    archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().header().mode().unwrap())
        .collect()
}

register_cases! {
    test_client_allowlist_permissions_checksum_and_no_overwrite,
    test_windows_zip_contains_only_exe,
    test_dashboard_contains_only_static_assets,
    test_identity_allowlist_excludes_vm_and_private_state,
    test_invalid_version_target_and_missing_binary_make_no_archive,
    test_server_contains_prebuilt_image_not_host_snapshots_or_secrets,
    test_bootstrap_accepts_windows_crlf_checkout,
}
