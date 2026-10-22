use anyhow::{Result, bail};
use kiln_server::{
    auth::Enrollment,
    enrollment::{EnrollmentEffects, EnrollmentPaths, Receipt, complete},
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[derive(Default)]
struct Fixture {
    commands: Vec<Vec<String>>,
    fail_command: bool,
    readiness_fail: bool,
    activations: Vec<String>,
}

impl EnrollmentEffects for Fixture {
    async fn systemctl(&mut self, args: &[&str]) -> Result<()> {
        self.commands
            .push(args.iter().map(|arg| (*arg).to_owned()).collect());
        if self.fail_command {
            bail!("fixture systemd failure")
        }
        Ok(())
    }

    async fn readiness(&mut self) -> Result<()> {
        if self.readiness_fail {
            bail!("fixture readiness failure")
        }
        Ok(())
    }

    async fn activate(&mut self, directory_token: &str) -> Result<()> {
        self.activations.push(directory_token.to_owned());
        Ok(())
    }
}

fn setup() -> (tempfile::TempDir, Receipt) {
    let root = tempfile::tempdir().unwrap();
    for path in [root.path().join("etc"), root.path().join("dropin")] {
        fs::create_dir(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let receipt = Receipt {
        enrollment: Enrollment {
            version: 1,
            issuer: "https://identity.example/".into(),
            owner_id: "owner".into(),
            server_id: "server".into(),
            tenant_id: String::new(),
        },
        origin: "https://server.example/".into(),
        ca_pem: String::new(),
        directory_token: "one-time-directory-token".into(),
        local_origin: String::new(),
        login_token: String::new(),
    };
    let bytes = serde_json::to_vec(&receipt).unwrap();
    fs::write(root.path().join("etc/enrollment.pending.json"), bytes).unwrap();
    fs::set_permissions(
        root.path().join("etc/enrollment.pending.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    (root, receipt)
}

async fn finish(root: &tempfile::TempDir, receipt: &Receipt, fixture: &mut Fixture) -> Result<()> {
    complete(
        receipt,
        EnrollmentPaths {
            etc: &root.path().join("etc"),
            dropin: &root.path().join("dropin"),
        },
        unsafe { libc::getegid() },
        unsafe { libc::getegid() },
        fixture,
    )
    .await
}

#[tokio::test]
async fn persistence_failure_happens_before_commands_or_activation() {
    let (root, receipt) = setup();
    fs::write(root.path().join("etc/directory.token"), b"conflict").unwrap();
    fs::set_permissions(
        root.path().join("etc/directory.token"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let mut fixture = Fixture::default();

    assert!(finish(&root, &receipt, &mut fixture).await.is_err());
    assert!(fixture.commands.is_empty());
    assert!(fixture.activations.is_empty());
    assert!(root.path().join("etc/enrollment.pending.json").exists());
}

#[tokio::test]
async fn failed_readiness_retains_receipt_and_only_restarts_api() {
    let (root, receipt) = setup();
    let mut fixture = Fixture {
        readiness_fail: true,
        ..Fixture::default()
    };

    assert!(finish(&root, &receipt, &mut fixture).await.is_err());
    assert_eq!(
        fixture.commands,
        [
            vec!["daemon-reload".to_owned()],
            vec!["restart".to_owned(), "kiln-api.service".to_owned()],
        ]
    );
    assert!(fixture.activations.is_empty());
    assert!(root.path().join("etc/enrollment.pending.json").exists());
}

#[tokio::test]
async fn failed_systemd_retains_receipt_without_readiness_or_activation() {
    let (root, receipt) = setup();
    let mut fixture = Fixture {
        fail_command: true,
        ..Fixture::default()
    };

    assert!(finish(&root, &receipt, &mut fixture).await.is_err());
    assert_eq!(fixture.commands, [vec!["daemon-reload".to_owned()]]);
    assert!(fixture.activations.is_empty());
    assert!(root.path().join("etc/enrollment.pending.json").exists());
}

#[tokio::test]
async fn resume_is_idempotent_and_activates_saved_credential() {
    let (root, receipt) = setup();
    let mut failed = Fixture {
        readiness_fail: true,
        ..Fixture::default()
    };
    assert!(finish(&root, &receipt, &mut failed).await.is_err());

    let mut resumed = Fixture::default();
    finish(&root, &receipt, &mut resumed).await.unwrap();
    assert_eq!(
        resumed.activations.as_slice(),
        std::slice::from_ref(&receipt.directory_token)
    );
    assert!(!root.path().join("etc/enrollment.pending.json").exists());
    assert_eq!(
        fs::read(root.path().join("etc/directory.token")).unwrap(),
        receipt.directory_token.as_bytes()
    );
    let dropin = fs::read(root.path().join("dropin/identity.conf")).unwrap();
    assert_eq!(
        dropin,
        b"[Service]\nStateDirectory=kiln-api\nStateDirectoryMode=0700\n"
    );
    assert!(!String::from_utf8(dropin).unwrap().contains("ExecStart"));
    assert!(!root.path().join("etc/login.token").exists());
}

#[tokio::test]
async fn login_token_is_group_readable_and_directory_token_stays_private() {
    let (root, mut receipt) = setup();
    receipt.login_token = "fixture-login".into();
    let mut fixture = Fixture::default();
    finish(&root, &receipt, &mut fixture).await.unwrap();
    let login = root.path().join("etc/login.token");
    let directory = root.path().join("etc/directory.token");
    assert_eq!(fs::read(&login).unwrap(), b"fixture-login");
    assert_eq!(
        fs::metadata(&login).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn a_missing_app_login_route_yields_an_empty_token() {
    let missing = reqwest::Response::from(
        axum::http::Response::builder()
            .status(404)
            .body(reqwest::Body::from("missing"))
            .unwrap(),
    );
    let token = kiln_server::enrollment::login_token_from_response(missing)
        .await
        .unwrap();
    assert!(token.is_empty());
    let issued = reqwest::Response::from(
        axum::http::Response::builder()
            .status(200)
            .body(reqwest::Body::from(r#"{"login_token":"fixture-login"}"#))
            .unwrap(),
    );
    assert_eq!(
        kiln_server::enrollment::login_token_from_response(issued)
            .await
            .unwrap(),
        "fixture-login"
    );
    let refused = reqwest::Response::from(
        axum::http::Response::builder()
            .status(401)
            .body(reqwest::Body::from("{}"))
            .unwrap(),
    );
    assert!(
        kiln_server::enrollment::login_token_from_response(refused)
            .await
            .is_err()
    );
}

#[test]
fn enrollment_is_explicit_and_does_not_change_legacy_serve_contract() {
    let exe = env!("CARGO_BIN_EXE_kiln-api");
    let version = Command::new(exe).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
    let help = Command::new(exe)
        .args(["enroll", "--help"])
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("--resume") && text.contains("--issuer"));
    let invalid = Command::new(exe).arg("enroll").output().unwrap();
    assert!(!invalid.status.success());
    let serve = Command::new(exe)
        .args([
            "--host-socket",
            "/nonexistent/host.sock",
            "--token-file",
            "/nonexistent/token",
            "--tls-cert",
            "/nonexistent/cert",
            "--tls-key",
            "/nonexistent/key",
        ])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&serve.stderr).contains("unexpected argument"));
}
