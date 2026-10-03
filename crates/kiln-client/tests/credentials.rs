use kiln_api::auth::ServerDescriptor;
use kiln_client::{
    credentials::{CredentialStore, Credentials},
    profile::{CentralProfile, DirectProfile, Profile},
};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn profiles_are_strict_and_legacy_json_is_unchanged() {
    let legacy = r#"{"url":"https://example.test","token_file":"/tmp/token","ca_file":null}"#;
    let profile: Profile = serde_json::from_str(legacy).unwrap();
    assert_eq!(
        serde_json::to_value(profile).unwrap(),
        serde_json::from_str::<serde_json::Value>(legacy).unwrap()
    );
    assert!(
        serde_json::from_str::<Profile>(
            r#"{"url":"https://x","token_file":"x","ca_file":null,"issuer":"https://i"}"#
        )
        .is_err()
    );
}

#[test]
fn central_profiles_contain_no_secret_and_validate_origins_and_paths() {
    let root = tempfile::tempdir().unwrap();
    let value = Profile::Central(CentralProfile {
        issuer: "https://identity.example".into(),
        server: ServerDescriptor {
            id: "srv_x".into(),
            name: "one".into(),
            origin: "https://server.example".into(),
            ca_pem: "pem".into(),
            revision: 1,
        },
        credential_file: root.path().join("credential.json"),
    });
    let text = serde_json::to_string(&value).unwrap();
    assert!(!text.contains("refresh_token"));
    assert!(value.validate().is_ok());
    let bad: Profile =
        serde_json::from_str(&text.replace("https://identity.example", "http://identity.example"))
            .unwrap();
    assert!(bad.validate().is_err());
}

#[cfg(unix)]
#[test]
fn credential_storage_is_private_atomic_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("private/credentials.json");
    let store = CredentialStore::new(path.clone());
    let credentials = Credentials {
        issuer: "https://identity.example".into(),
        access_token: "access".into(),
        access_expires_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300,
        refresh_token: "refresh".into(),
        automation_key_file: None,
        server_tokens: Default::default(),
    };
    store.save(&credentials).unwrap();
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(store.load().unwrap().unwrap().refresh_token, "refresh");
    let victim = root.path().join("victim");
    fs::write(&victim, "safe").unwrap();
    let link = root.path().join("link");
    symlink(&victim, &link).unwrap();
    assert!(CredentialStore::new(link).save(&credentials).is_err());
    assert_eq!(fs::read_to_string(victim).unwrap(), "safe");
}

#[allow(dead_code)]
fn direct_type_remains_available(p: DirectProfile) -> Profile {
    Profile::Direct(p)
}

#[cfg(unix)]
#[test]
fn failed_login_preserves_existing_profile_and_credentials() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("profiles.json");
    let mut central = CentralProfile {
        issuer: "https://identity.example".into(),
        server: ServerDescriptor {
            id: "srv_one".into(),
            name: "one".into(),
            origin: "https://host.example".into(),
            ca_pem: "old-ca".into(),
            revision: 1,
        },
        credential_file: root.path().join("private/old.json"),
    };
    let mut credentials = Credentials {
        issuer: central.issuer.clone(),
        access_token: "old-access".into(),
        access_expires_at: 1000,
        refresh_token: "old-refresh".into(),
        automation_key_file: None,
        server_tokens: Default::default(),
    };
    kiln_client::profile::save_login(&path, "default", central.clone(), &credentials).unwrap();
    let original = fs::read(&path).unwrap();
    let old_file = central.credential_file.clone();
    central.credential_file = root.path().join("private/new.json");
    central.server.ca_pem = "unapproved-ca".into();
    credentials.refresh_token = "new-refresh".into();
    assert!(
        kiln_client::profile::save_login(&path, "default", central.clone(), &credentials).is_err()
    );
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(
        CredentialStore::new(old_file)
            .load()
            .unwrap()
            .unwrap()
            .refresh_token,
        "old-refresh"
    );
    assert!(!central.credential_file.exists());
}
