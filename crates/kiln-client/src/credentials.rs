use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedToken {
    pub token: String,
    pub expires_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub issuer: String,
    pub access_token: String,
    pub access_expires_at: u64,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_key_file: Option<PathBuf>,
    #[serde(default)]
    pub server_tokens: BTreeMap<String, CachedToken>,
}

#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn parent(&self) -> &Path {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }

    fn secure_parent(&self) -> Result<()> {
        let parent = self.parent();
        #[cfg(windows)]
        {
            windows_acl::ensure_private_directory(parent)?;
        }
        #[cfg(not(windows))]
        if parent.exists() {
            reject_link(parent)?;
        } else {
            let mut b = fs::DirBuilder::new();
            b.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            b.create(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let m = fs::symlink_metadata(parent)?;
            ensure!(
                m.is_dir() && m.uid() == unsafe { libc::geteuid() },
                "credential directory is not owned by current user"
            );
            ensure!(
                m.permissions().mode() & 0o077 == 0,
                "credential directory is accessible by other users"
            );
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Credentials>> {
        self.secure_parent()?;
        if !self.path.try_exists()? && fs::symlink_metadata(&self.path).is_err() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&read_private(
            &self.path,
            1024 * 1024,
        )?)?))
    }

    pub fn save(&self, value: &Credentials) -> Result<()> {
        self.secure_parent()?;
        if fs::symlink_metadata(&self.path).is_ok() {
            reject_link(&self.path)?;
        }
        #[cfg(windows)]
        {
            let bytes = serde_json::to_vec(value)?;
            windows_acl::replace_private(&self.path, &bytes)?;
        }
        #[cfg(not(windows))]
        let mut tmp = tempfile::NamedTempFile::new_in(self.parent())?;
        #[cfg(not(windows))]
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tmp.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            tmp.write_all(&serde_json::to_vec(value)?)?;
            tmp.as_file().sync_all()?;
            tmp.persist(&self.path).map_err(|e| e.error)?;
            #[cfg(unix)]
            fs::File::open(self.parent())?.sync_all()?;
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        self.secure_parent()?;
        if fs::symlink_metadata(&self.path).is_ok() {
            reject_link(&self.path)?;
            fs::remove_file(&self.path)?;
            #[cfg(unix)]
            fs::File::open(self.parent())?.sync_all()?;
        }
        Ok(())
    }

    pub async fn acquire(&self) -> Result<fs::File> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.lock()).await?
    }

    pub fn lock(&self) -> Result<fs::File> {
        self.secure_parent()?;
        let p = self.parent().join(".credentials.lock");
        if fs::symlink_metadata(&p).is_ok() {
            reject_link(&p)?;
        }
        #[cfg(windows)]
        let f = windows_acl::open_private_lock(&p)?;
        #[cfg(not(windows))]
        {
            let mut options = fs::OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let f = options.open(p)?;
            check_file(&f, 1024)?;
            f.lock().context("could not lock credentials")?;
            Ok(f)
        }
        #[cfg(windows)]
        {
            check_file(&f, 1024)?;
            f.lock().context("could not lock credentials")?;
            Ok(f)
        }
    }
}

fn reject_link(path: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "refusing symlink in credential storage"
    );
    #[cfg(windows)]
    windows_acl::reject_reparse(path)?;
    Ok(())
}

fn check_file(file: &fs::File, limit: u64) -> Result<()> {
    let m = file.metadata()?;
    ensure!(m.is_file() && m.len() <= limit, "invalid credential file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "credential file is not private"
        );
    }
    #[cfg(windows)]
    windows_acl::check_private_path(file)?;
    Ok(())
}

#[cfg(windows)]
mod windows_acl {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::{ffi::OsStrExt, fs::MetadataExt, io::FromRawHandle},
        ptr,
        sync::atomic::{AtomicU64, Ordering},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetSecurityDescriptorControl,
            GetSecurityDescriptorOwner, GetTokenInformation, IsValidAcl, IsWellKnownSid,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, TOKEN_QUERY,
            TOKEN_USER, TokenUser, WinLocalSystemSid,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, GetFileInformationByHandleEx,
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken},
    };

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    struct Local(*mut c_void);
    impl Drop for Local {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    fn private_sd() -> Result<(Local, PSECURITY_DESCRIPTOR)> {
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            ensure!(
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0,
                "cannot inspect current Windows user token"
            );
            struct Token(HANDLE);
            impl Drop for Token {
                fn drop(&mut self) {
                    unsafe {
                        CloseHandle(self.0);
                    }
                }
            }
            let _token = Token(token);
            let mut size = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            ensure!(size > 0, "cannot size Windows user token");
            let mut buf = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            ensure!(
                GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size, &mut size)
                    != 0,
                "cannot read Windows user token"
            );
            let sid = (*(buf.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            let mut sid_text = ptr::null_mut();
            ensure!(
                ConvertSidToStringSidW(sid, &mut sid_text) != 0,
                "cannot format Windows user SID"
            );
            let sid_local = Local(sid_text.cast());
            let len = (0..).take_while(|&i| *sid_text.add(i) != 0).count();
            let sid_string = String::from_utf16(std::slice::from_raw_parts(sid_text, len))?;
            drop(sid_local);
            let sddl: Vec<u16> = format!("O:{sid_string}D:P(A;;FA;;;{sid_string})(A;;FA;;;SY)")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut sd = ptr::null_mut();
            ensure!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut sd,
                    ptr::null_mut()
                ) != 0,
                "cannot construct private Windows ACL"
            );
            Ok((Local(sd), sd))
        }
    }

    pub fn reject_reparse(path: &Path) -> Result<()> {
        ensure!(
            fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "refusing reparse point in credential storage"
        );
        Ok(())
    }

    pub(super) fn check_private(path: &Path) -> Result<()> {
        reject_reparse(path)?;
        use std::os::windows::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        check_private_path(&file)
    }

    pub fn check_private_path(file: &fs::File) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            let handle = file.as_raw_handle() as HANDLE;
            let mut info: FILE_ATTRIBUTE_TAG_INFO = std::mem::zeroed();
            ensure!(
                GetFileInformationByHandleEx(
                    handle,
                    FileAttributeTagInfo,
                    (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                    std::mem::size_of_val(&info) as u32
                ) != 0,
                "cannot inspect credential file"
            );
            ensure!(
                info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "refusing credential reparse point"
            );
            let mut owner: PSID = ptr::null_mut();
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
            let status = GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut sd,
            );
            let _local = Local(sd);
            ensure!(
                status == 0 && !sd.is_null() && !dacl.is_null() && !owner.is_null(),
                "credential path has no private ACL"
            );
            let (_expected_local, expected) = private_sd()?;
            let mut user = ptr::null_mut();
            let mut defaulted = 0;
            ensure!(
                GetSecurityDescriptorOwner(expected, &mut user, &mut defaulted) != 0
                    && !user.is_null(),
                "missing current user SID"
            );
            ensure!(
                EqualSid(owner, user) != 0,
                "credential owner differs from current user"
            );
            let mut control = 0;
            let mut revision = 0;
            ensure!(
                GetSecurityDescriptorControl(sd, &mut control, &mut revision) != 0
                    && control & SE_DACL_PROTECTED != 0
                    && IsValidAcl(dacl) != 0,
                "credential ACL must be protected and valid"
            );
            for index in 0..(*dacl).AceCount {
                let mut raw = ptr::null_mut();
                ensure!(
                    GetAce(dacl, u32::from(index), &mut raw) != 0 && !raw.is_null(),
                    "invalid credential ACE"
                );
                let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
                // Ordinary allow ACEs only; unknown/object/callback ACEs fail closed.
                ensure!(
                    ace.Header.AceType == 0
                        && usize::from(ace.Header.AceSize)
                            >= std::mem::size_of::<ACCESS_ALLOWED_ACE>(),
                    "unsupported credential ACE"
                );
                let sid = ptr::addr_of!(ace.SidStart).cast_mut().cast();
                ensure!(
                    EqualSid(sid, user) != 0 || IsWellKnownSid(sid, WinLocalSystemSid) != 0,
                    "credential ACL grants access to another principal"
                );
            }
        }
        Ok(())
    }

    pub fn ensure_private_directory(path: &Path) -> Result<()> {
        if path.try_exists()? {
            ensure!(
                fs::metadata(path)?.is_dir(),
                "credential directory is not a directory"
            );
            return check_private(path);
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !parent.exists() {
                ensure_private_directory(parent)?;
            }
        }
        let (sd_local, sd) = private_sd()?;
        let mut sa = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        ensure!(
            unsafe { CreateDirectoryW(wide(path).as_ptr(), &mut sa) } != 0,
            "cannot create private credential directory"
        );
        drop(sd_local);
        check_private(path)
    }

    pub fn open_private_lock(path: &Path) -> Result<fs::File> {
        if path.try_exists()? {
            check_private(path)?;
        }
        let (sd_local, sd) = private_sd()?;
        let mut sa = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateFileW(
                wide(path).as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &mut sa,
                windows_sys::Win32::Storage::FileSystem::OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        drop(sd_local);
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "cannot open private credential lock"
        );
        let file = unsafe { fs::File::from_raw_handle(handle) };
        check_private(path)?;
        Ok(file)
    }

    pub fn create_new_private(path: &Path) -> Result<fs::File> {
        let (sd_local, sd) = private_sd()?;
        let mut sa = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateFileW(
                wide(path).as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                &mut sa,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "cannot create private credential temporary file"
        );
        drop(sd_local);
        let file = unsafe { fs::File::from_raw_handle(handle) };
        check_private_path(&file)?;
        Ok(file)
    }

    pub fn replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
        if path.try_exists()? {
            check_private(path)?;
        }
        let name = format!(
            ".credentials.{}.{}.tmp",
            unsafe { GetCurrentProcessId() },
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        let temp = path.parent().unwrap_or(Path::new(".")).join(name);
        let mut file = create_new_private(&temp)?;
        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            check_private_path(&file)?;
            drop(file);
            ensure!(
                unsafe {
                    MoveFileExW(
                        wide(&temp).as_ptr(),
                        wide(path).as_ptr(),
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                } != 0,
                "cannot atomically replace credentials"
            );
            check_private(path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    reject_link(path)?;
    #[cfg(windows)]
    windows_acl::check_private(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    check_file(&file, limit)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "credential file too large");
    Ok(bytes)
}

/// Create a new secret file; never overwrite an existing key.
pub fn write_secret(path: &Path, value: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    #[cfg(windows)]
    let mut file = windows_acl::create_new_private(path)?;
    #[cfg(not(windows))]
    let mut file = {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)?
    };
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn read_secret(path: &Path) -> Result<String> {
    let text = String::from_utf8(read_private(path, 8192)?)?;
    let token = text.trim();
    ensure!(
        !token.is_empty()
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~+/=".contains(&b)),
        "invalid credential"
    );
    Ok(token.to_owned())
}
