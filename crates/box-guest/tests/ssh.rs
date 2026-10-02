use box_guest::ssh::SessionFiles;
use box_guest::{Agent, FakeInitializer};
use box_protocol::{SshConnect, write_frame};
use std::sync::Arc;

const KEY_A: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const KEY_B: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB";

#[test]
fn session_files_admit_only_the_sessions_key_and_are_removed_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let first = SessionFiles::create(root.path(), KEY_A, "/host/key").unwrap();
    let second = SessionFiles::create(root.path(), KEY_B, "/host/key").unwrap();
    assert_ne!(first.directory(), second.directory());
    assert_eq!(
        std::fs::read_to_string(first.authorized_keys()).unwrap(),
        format!("{KEY_A}\n")
    );
    assert_eq!(
        std::fs::read_to_string(second.authorized_keys()).unwrap(),
        format!("{KEY_B}\n")
    );
    let first_directory = first.directory().to_path_buf();
    let config = std::fs::read_to_string(first.config()).unwrap();
    assert!(config.contains("PasswordAuthentication no"));
    assert!(config.contains("KbdInteractiveAuthentication no"));
    assert!(config.contains("AllowAgentForwarding no"));
    assert!(config.contains("X11Forwarding no"));
    assert!(config.contains("AllowTcpForwarding local"));
    assert!(config.contains("PermitRootLogin prohibit-password"));
    drop(first);
    assert!(!first_directory.exists());
    assert!(second.directory().exists());
}

#[test]
fn session_files_reject_malformed_or_injectable_keys() {
    let root = tempfile::tempdir().unwrap();
    for key in [
        "not-a-key".to_owned(),
        format!("{KEY_A} root@host"),
        format!("{KEY_A}\ncommand=bad"),
    ] {
        assert!(SessionFiles::create(root.path(), &key, "/host/key").is_err());
    }
}

#[test]
fn generated_configuration_is_accepted_by_openssh() {
    if !std::path::Path::new("/usr/sbin/sshd").exists() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let host_key = root.path().join("host_key");
    assert!(
        std::process::Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&host_key)
            .status()
            .unwrap()
            .success()
    );
    let files = SessionFiles::create(root.path(), KEY_A, host_key.to_str().unwrap()).unwrap();
    assert!(
        std::process::Command::new("/usr/sbin/sshd")
            .args(["-t", "-f"])
            .arg(files.config())
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn ssh_admission_denies_pre_init_and_malformed_keys() {
    let agent = Agent::new(Arc::new(FakeInitializer::default()), 1);
    for key in [KEY_A, "not-a-key"] {
        let (mut client, server) = tokio::io::duplex(4096);
        write_frame(
            &mut client,
            &SshConnect {
                public_key: key.into(),
            },
        )
        .await
        .unwrap();
        assert!(box_guest::ssh::serve_session(server, &agent).await.is_err());
    }
}
