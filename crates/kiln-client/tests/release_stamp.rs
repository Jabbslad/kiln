//! A release tag can differ from Cargo.toml. The packaged binary must report the tag.
//!
//! The stamp scripts run on the Ubuntu release runner (GNU tar). Client tests
//! on macOS do not execute this proof.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(program: &str, args: &[&str]) -> std::process::Output {
    Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{program} {args:?}: {error}"))
}

fn assert_ok(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn version_of(binary: &Path) -> String {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", binary.display()));
    assert_ok(&output, &format!("{} --version", binary.display()));
    String::from_utf8(output.stdout).unwrap()
}

fn sha256(path: &Path) -> String {
    let output = run("sha256sum", &[path.to_str().unwrap()]);
    assert_ok(&output, "sha256sum");
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn write_sidecar(archive: &Path) {
    let digest = sha256(archive);
    let name = archive.file_name().unwrap().to_str().unwrap();
    fs::write(
        PathBuf::from(format!("{}.sha256", archive.display())),
        format!("{digest}  {name}\n"),
    )
    .unwrap();
}

#[test]
fn a_tag_that_differs_from_cargo_toml_is_what_the_binary_reports() {
    let cargo = env!("CARGO_PKG_VERSION");
    let tag = "9.9.9";
    assert_ne!(cargo, tag, "the fixture tag must differ from Cargo.toml");

    let source = PathBuf::from(env!("CARGO_BIN_EXE_kiln"));
    let bytes = fs::read(&source).unwrap();
    let marker = b"KILNRELSLOT=";
    let copies = bytes
        .windows(marker.len())
        .filter(|window| *window == marker)
        .count();
    assert_eq!(copies, 1, "release binaries need one version slot");
    assert_eq!(version_of(&source), format!("kiln {cargo}\n"));

    let root = std::env::temp_dir().join(format!("kiln-stamp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let stamped = root.join("kiln");
    fs::copy(&source, &stamped).unwrap();
    let stamp = repo_root().join("scripts/stamp-release-version.sh");
    let output = run(
        "sh",
        &[stamp.to_str().unwrap(), stamped.to_str().unwrap(), tag],
    );
    assert_ok(&output, "stamp binary to a tag that is not Cargo.toml");
    assert_eq!(version_of(&stamped), format!("kiln {tag}\n"));

    let dist = root.join("dist");
    fs::create_dir_all(&dist).unwrap();
    let pack = root.join("pack");
    fs::create_dir_all(pack.join("bin")).unwrap();
    fs::copy(&source, pack.join("bin/kiln")).unwrap();
    fs::write(
        pack.join("release.json"),
        format!(r#"{{"version": "v{cargo}", "target": "x86_64-unknown-linux-gnu"}}"#),
    )
    .unwrap();
    let server = dist.join(format!(
        "kiln-server-v{cargo}-x86_64-unknown-linux-gnu.tar.gz"
    ));
    let packed = run(
        "tar",
        &[
            "-C",
            pack.to_str().unwrap(),
            "-czf",
            server.to_str().unwrap(),
            "bin/kiln",
            "release.json",
        ],
    );
    assert_ok(&packed, "pack server archive");
    write_sidecar(&server);

    let client_dir = root.join("client");
    fs::create_dir_all(&client_dir).unwrap();
    fs::copy(&source, client_dir.join("kiln")).unwrap();
    let client = dist.join(format!("kiln-v{cargo}-x86_64-unknown-linux-gnu.tar.gz"));
    let packed = run(
        "tar",
        &[
            "-C",
            client_dir.to_str().unwrap(),
            "-czf",
            client.to_str().unwrap(),
            "kiln",
        ],
    );
    assert_ok(&packed, "pack client archive");
    write_sidecar(&client);

    let dist_stamp = repo_root().join("scripts/stamp-release-dist.sh");
    let output = run(
        "sh",
        &[
            dist_stamp.to_str().unwrap(),
            dist.to_str().unwrap(),
            &format!("v{cargo}"),
            &format!("v{tag}"),
        ],
    );
    assert_ok(
        &output,
        "stamp package from Cargo.toml version onto the tag",
    );

    assert!(!server.exists(), "old server archive name remains");
    assert!(!client.exists(), "old client archive name remains");
    let server = dist.join(format!(
        "kiln-server-v{tag}-x86_64-unknown-linux-gnu.tar.gz"
    ));
    let client = dist.join(format!("kiln-v{tag}-x86_64-unknown-linux-gnu.tar.gz"));
    assert!(server.is_file());
    assert!(client.is_file());
    for archive in [&server, &client] {
        let sidecar = PathBuf::from(format!("{}.sha256", archive.display()));
        let check = Command::new("sha256sum")
            .arg("--check")
            .arg(&sidecar)
            .current_dir(&dist)
            .output()
            .unwrap();
        assert_ok(&check, "checksum sidecar");
    }

    let extracted = root.join("extracted");
    fs::create_dir_all(&extracted).unwrap();
    assert_ok(
        &run(
            "tar",
            &[
                "-xzf",
                server.to_str().unwrap(),
                "-C",
                extracted.to_str().unwrap(),
            ],
        ),
        "extract stamped server",
    );
    assert_eq!(
        version_of(&extracted.join("bin/kiln")),
        format!("kiln {tag}\n")
    );
    let release = fs::read_to_string(extracted.join("release.json")).unwrap();
    assert!(
        release.contains(&format!("\"version\": \"v{tag}\"")),
        "{release}"
    );
    assert!(
        !release.contains(&format!("\"version\": \"v{cargo}\"")),
        "{release}"
    );

    let client_out = root.join("client-out");
    fs::create_dir_all(&client_out).unwrap();
    assert_ok(
        &run(
            "tar",
            &[
                "-xzf",
                client.to_str().unwrap(),
                "-C",
                client_out.to_str().unwrap(),
            ],
        ),
        "extract stamped client",
    );
    assert_eq!(
        version_of(&client_out.join("kiln")),
        format!("kiln {tag}\n")
    );

    let _ = fs::remove_dir_all(&root);
}
