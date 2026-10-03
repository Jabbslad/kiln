#![cfg(windows)]

use kiln_client::credentials::{CachedToken, CredentialStore, Credentials};
use std::{collections::BTreeMap, ffi::c_void, fs, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        ACL,
        Authorization::{
            BuildTrusteeWithSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetEffectiveRightsFromAclW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
            SetNamedSecurityInfoW, TRUSTEE_W,
        },
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, WinWorldSid,
    },
};

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

unsafe fn dacl_from_sddl(sddl: &str) -> (PSECURITY_DESCRIPTOR, *mut ACL) {
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut sd = ptr::null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut sd,
                ptr::null_mut(),
            )
        },
        0
    );
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = ptr::null_mut();
    assert_ne!(
        unsafe {
            windows_sys::Win32::Security::GetSecurityDescriptorDacl(
                sd,
                &mut present,
                &mut dacl,
                &mut defaulted,
            )
        },
        0
    );
    assert_ne!(present, 0);
    (sd, dacl)
}

fn make_permissive(path: &Path) {
    unsafe {
        let (sd, dacl) = dacl_from_sddl("D:P(A;;FA;;;WD)");
        let mut name = wide(path);
        assert_eq!(
            SetNamedSecurityInfoW(
                name.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            ),
            0
        );
        LocalFree(sd.cast::<c_void>());
    }
}

fn dacl(path: &Path) -> (*mut c_void, *mut ACL) {
    unsafe {
        let mut acl = ptr::null_mut();
        let mut sd = ptr::null_mut();
        assert_eq!(
            GetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut sd,
            ),
            0
        );
        (sd, acl)
    }
}

#[test]
fn created_credentials_have_no_effective_everyone_access_and_permissive_files_fail() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("private").join("credentials.json");
    let store = CredentialStore::new(path.clone());
    let credentials = Credentials {
        issuer: "https://identity.example".into(),
        access_token: "secret".into(),
        access_expires_at: 1,
        refresh_token: "refresh".into(),
        automation_key_file: None,
        server_tokens: BTreeMap::<String, CachedToken>::new(),
    };
    let _lock = store.lock().unwrap();
    store.save(&credentials).unwrap();
    store.save(&credentials).unwrap();
    assert_eq!(store.load().unwrap().unwrap().refresh_token, "refresh");

    // The World SID is absent from the effective DACL produced by the store.
    unsafe {
        let (sd, acl) = dacl(&path);
        let mut world = [0u8; 68];
        let mut world_len = world.len() as u32;
        assert_ne!(
            CreateWellKnownSid(
                WinWorldSid,
                ptr::null_mut(),
                world.as_mut_ptr().cast(),
                &mut world_len
            ),
            0
        );
        let mut trustee = TRUSTEE_W::default();
        BuildTrusteeWithSidW(&mut trustee, world.as_mut_ptr().cast());
        let mut rights = 0;
        assert_eq!(GetEffectiveRightsFromAclW(acl, &trustee, &mut rights), 0);
        assert_eq!(rights, 0, "Everyone has effective access to credentials");
        LocalFree(sd);
    }

    make_permissive(&path);
    assert!(store.load().is_err());
    assert!(store.save(&credentials).is_err());
    assert!(fs::read(&path).is_ok());
    make_permissive(path.parent().unwrap());
    assert!(store.lock().is_err());
}
