use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::{fs, io::Read};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootMode {
    #[default]
    Init,
    Systemd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub architecture: String,
    pub kernel_path: PathBuf,
    pub kernel_sha256: String,
    pub rootfs_path: PathBuf,
    pub rootfs_sha256: String,
    pub agent_protocol_version: u32,
    #[serde(default)]
    pub boot_mode: BootMode,
}

impl Manifest {
    pub fn boot_args(&self) -> &'static str {
        match self.boot_mode {
            BootMode::Init => {
                "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/init quiet"
            }
            BootMode::Systemd => {
                "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/box-init quiet"
            }
        }
    }
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn load(path: &Path) -> Result<Manifest> {
    let path = fs::canonicalize(path)?;
    let mut manifest: Manifest = serde_json::from_slice(&fs::read(&path)?)?;
    if manifest.schema_version != 1
        || manifest.agent_protocol_version != 1
        || manifest.architecture != "x86_64"
    {
        return Err(Error::Invalid(
            "unsupported image schema, architecture or agent protocol".into(),
        ));
    }
    let directory = path
        .parent()
        .ok_or_else(|| Error::Invalid("image has no parent".into()))?;
    manifest.kernel_path = fs::canonicalize(directory.join(&manifest.kernel_path))?;
    manifest.rootfs_path = fs::canonicalize(directory.join(&manifest.rootfs_path))?;
    for (path, expected) in [
        (&manifest.kernel_path, &manifest.kernel_sha256),
        (&manifest.rootfs_path, &manifest.rootfs_sha256),
    ] {
        if sha256(path)? != *expected {
            return Err(Error::Invalid(format!(
                "checksum mismatch: {}",
                path.display()
            )));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn boot_mode_is_explicit_and_legacy_images_keep_their_init() {
        let value = serde_json::json!({
            "schema_version":1,"architecture":"x86_64","kernel_path":"kernel",
            "kernel_sha256":"abc","rootfs_path":"disk","rootfs_sha256":"def",
            "agent_protocol_version":1
        });
        let legacy: Manifest = serde_json::from_value(value.clone()).unwrap();
        assert!(legacy.boot_args().contains("init=/sbin/init"));
        let mut ubuntu = value;
        ubuntu["boot_mode"] = "systemd".into();
        let manifest: Manifest = serde_json::from_value(ubuntu.clone()).unwrap();
        assert!(manifest.boot_args().contains("init=/sbin/box-init"));
        ubuntu["boot_mode"] = "init=/bin/sh".into();
        assert!(serde_json::from_value::<Manifest>(ubuntu).is_err());
    }

    #[test]
    fn resolves_paths_and_rejects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        // Independently known SHA-256 of the ASCII bytes "abc".
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        fs::write(dir.path().join("kernel"), b"abc").unwrap();
        fs::write(dir.path().join("disk"), b"abc").unwrap();
        let manifest = Manifest {
            schema_version: 1,
            architecture: "x86_64".into(),
            kernel_path: "kernel".into(),
            kernel_sha256: hash.into(),
            rootfs_path: "disk".into(),
            rootfs_sha256: hash.into(),
            agent_protocol_version: 1,
            boot_mode: BootMode::Init,
        };
        let path = dir.path().join("image.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(load(&path).unwrap().rootfs_path, dir.path().join("disk"));
        fs::write(dir.path().join("disk"), b"abd").unwrap();
        assert!(load(&path).unwrap_err().to_string().contains("checksum"));
    }
}
