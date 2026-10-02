use anyhow::{Context, Result, ensure};
use kiln_client::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub url: String,
    pub token_file: PathBuf,
    pub ca_file: Option<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    pub profiles: BTreeMap<String, Profile>,
}

impl Profile {
    pub fn client(&self) -> Result<Client> {
        let token =
            kiln_api::read_token(&self.token_file).context("could not read private token file")?;
        let ca = self
            .ca_file
            .as_ref()
            .map(fs::read)
            .transpose()
            .context("could not read CA file")?;
        Client::new(&self.url, &token, ca.as_deref())
    }
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
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
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
