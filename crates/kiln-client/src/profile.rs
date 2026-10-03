use crate::{Client, auth::Session};
use anyhow::{Context, Result, ensure};
use kiln_api::auth::ServerDescriptor;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectProfile {
    pub url: String,
    pub token_file: PathBuf,
    pub ca_file: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CentralProfile {
    pub issuer: String,
    pub server: ServerDescriptor,
    pub credential_file: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Profile {
    Direct(DirectProfile),
    Central(CentralProfile),
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    pub profiles: BTreeMap<String, Profile>,
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Direct(p) => {
                validate_origin(&p.url)?;
            }
            Self::Central(p) => {
                validate_origin(&p.issuer)?;
                validate_origin(&p.server.origin)?;
                ensure!(
                    p.credential_file.is_absolute(),
                    "credential path must be absolute"
                );
            }
        }
        Ok(())
    }
    pub fn client(&self) -> Result<Client> {
        let Self::Direct(this) = self else {
            let Self::Central(p) = self else {
                unreachable!()
            };
            return Client::with_session(p, Session::new(p.clone())?);
        };
        let token =
            kiln_api::read_token(&this.token_file).context("could not read private token file")?;
        let ca = this
            .ca_file
            .as_ref()
            .map(fs::read)
            .transpose()
            .context("could not read CA file")?;
        Client::new(&this.url, &token, ca.as_deref())
    }
}

fn validate_origin(value: &str) -> Result<()> {
    let u = reqwest::Url::parse(value)?;
    ensure!(
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "URL must be an HTTPS origin"
    );
    Ok(())
}

pub fn default_path() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME").or_else(|| std::env::var_os("APPDATA"))
    {
        return Ok(PathBuf::from(root).join("kiln/profiles.json"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("no home directory; pass --config")?;
    Ok(PathBuf::from(home).join(".config/kiln/profiles.json"))
}

pub fn load(path: &Path) -> Result<Profiles> {
    match fs::read(path) {
        Ok(bytes) => {
            let profiles: Profiles = serde_json::from_slice(&bytes)?;
            for profile in profiles.profiles.values() {
                profile.validate()?;
            }
            Ok(profiles)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Profiles::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn add(path: &Path, name: &str, profile: Profile) -> Result<()> {
    ensure!(kiln_api::valid_alias(name), "invalid profile name");
    profile.client()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    // Serialize concurrent profile edits; atomic replacement prevents partial JSON.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".profiles.lock"))?;
    lock.lock()?;
    let mut profiles = load(path)?;
    ensure!(
        !profiles.profiles.contains_key(name),
        "profile already exists; edit its entry in the config file or use another name"
    );
    profiles.profiles.insert(name.into(), profile);
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(&profiles)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn set(path: &Path, name: &str, profile: Profile) -> Result<()> {
    ensure!(kiln_api::valid_alias(name), "invalid profile name");
    profile.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(parent)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".profiles.lock"))?;
    lock.lock()?;
    let mut profiles = load(path)?;
    ensure!(
        !matches!(profiles.profiles.get(name), Some(Profile::Direct(_))),
        "refusing to replace a direct administrator profile"
    );
    if let (Some(Profile::Central(old)), Profile::Central(new)) =
        (profiles.profiles.get(name), &profile)
    {
        ensure!(old.issuer == new.issuer, "refusing changed identity issuer");
        if old.server.id == new.server.id {
            ensure!(
                old.server.ca_pem == new.server.ca_pem,
                "server CA changed; explicit re-enrollment is required"
            );
        }
    }
    profiles.profiles.insert(name.into(), profile);
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(&profiles)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// A new login has its own credential file. Rejected trust/profile changes must
/// never replace the previous profile's refresh secret.
pub fn save_login(
    path: &Path,
    name: &str,
    profile: CentralProfile,
    credentials: &crate::credentials::Credentials,
) -> Result<()> {
    let store = crate::credentials::CredentialStore::new(profile.credential_file.clone());
    let _lock = store.lock()?;
    ensure!(
        !profile.credential_file.try_exists()?,
        "login requires a new credential file"
    );
    store.save(credentials)?;
    if let Err(error) = set(path, name, Profile::Central(profile)) {
        store.clear()?;
        return Err(error);
    }
    Ok(())
}
