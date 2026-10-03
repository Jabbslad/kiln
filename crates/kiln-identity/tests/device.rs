use kiln_identity::Store;

#[cfg(unix)]
fn make_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(not(unix))]
fn make_private(_: &std::path::Path) {}

#[test]
fn store_persists_and_never_stores_raw_device_secret() {
    let dir = tempfile::tempdir().unwrap();
    make_private(dir.path());
    let path = dir.path().join("identity.db");
    let store = Store::open(&path).unwrap();
    let grant = store.create_device("laptop", "127.0.0.1", 100).unwrap();
    assert_ne!(grant.device_code, grant.user_code);
    assert_eq!(grant.expires_in, 600);
    assert_eq!(grant.interval, 5);
    drop(store);
    let bytes = std::fs::read(path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(&grant.device_code));
}

#[test]
fn approval_is_single_use_and_requires_a_live_account() {
    let store = Store::open_memory().unwrap();
    let account = store.seed_account("github", "42", "tester", 100).unwrap();
    let grant = store.create_device("laptop", "127.0.0.1", 100).unwrap();
    store
        .approve_code(&grant.user_code, &account, true, 101)
        .unwrap();
    assert!(
        store
            .approve_code(&grant.user_code, &account, true, 102)
            .is_err()
    );
    assert!(
        store
            .approve_code("NOT-A-CODE", &account, true, 102)
            .is_err()
    );
}

#[test]
fn two_connections_cannot_both_approve_one_code() {
    let dir = tempfile::tempdir().unwrap();
    make_private(dir.path());
    let path = dir.path().join("identity.db");
    let first = Store::open(&path).unwrap();
    let second = Store::open(&path).unwrap();
    let account = first.seed_account("github", "42", "tester", 100).unwrap();
    let grant = first.create_device("laptop", "127.0.0.1", 100).unwrap();
    first
        .approve_code(&grant.user_code, &account, true, 101)
        .unwrap();
    assert!(
        second
            .approve_code(&grant.user_code, &account, true, 101)
            .is_err()
    );
}

#[test]
fn provider_subjects_are_namespaced_and_never_linked_by_display_identity() {
    let store = Store::open_memory().unwrap();
    let github = store
        .seed_account("github", "101", "same@example.test", 100)
        .unwrap();
    let google = store
        .seed_account("google", "101", "same@example.test", 100)
        .unwrap();
    assert_ne!(github, google);

    assert_eq!(
        github,
        store.seed_account("github", "101", "renamed", 101).unwrap()
    );
}

#[test]
fn approval_expiry_is_exclusive_at_the_boundary() {
    let store = Store::open_memory().unwrap();
    let account = store.seed_account("github", "42", "tester", 100).unwrap();
    let before = store.create_device("before", "192.0.2.1", 100).unwrap();
    store
        .approve_code(&before.user_code, &account, true, 699)
        .unwrap();

    let boundary = store.create_device("boundary", "192.0.2.2", 100).unwrap();
    assert!(
        store
            .approve_code(&boundary.user_code, &account, true, 700)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn database_rejects_symlinks_public_files_and_unknown_schema() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    make_private(dir.path());
    let path = dir.path().join("identity.db");
    drop(Store::open(&path).unwrap());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let link = dir.path().join("link.db");
    symlink(&path, &link).unwrap();
    assert!(Store::open(&link).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Store::open(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
}
