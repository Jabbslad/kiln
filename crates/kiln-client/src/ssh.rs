use anyhow::{Context, Result, ensure};
#[cfg(unix)]
use std::process::Command;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn safe_config_value(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && !value.chars().any(char::is_control)
            && !value.contains(['%', '\'', '"', '\\', '$', '`']),
        "value contains characters unsafe for OpenSSH configuration"
    );
    Ok(())
}

#[cfg(unix)]
fn safe_profile(value: &str) -> Result<()> {
    ensure!(kiln_api::valid_alias(value), "invalid profile name");
    safe_config_value(value)
}

fn shell_quote(value: &str) -> Result<String> {
    safe_config_value(value)?;
    Ok(format!("'{value}'"))
}

pub fn proxy_command(config: &Path, profile: &str, executable: &str) -> Result<String> {
    let config = config.to_str().context("configuration path is not UTF-8")?;
    Ok([
        executable,
        "--config",
        config,
        "--profile",
        profile,
        "ssh-proxy",
    ]
    .into_iter()
    .map(shell_quote)
    .collect::<Result<Vec<_>>>()?
    .join(" "))
}

pub fn parse_copy_endpoint(value: &str) -> Result<Option<(String, String)>> {
    let Some((id, path)) = value.split_once(':') else {
        return Ok(None);
    };
    ensure!(
        kiln_api::valid_id(id),
        "remote endpoint must use a full 32-character box ID"
    );
    ensure!(
        path.starts_with('/') && !path.contains(['\n', '\r']),
        "remote path must be absolute and contain no newlines"
    );
    Ok(Some((id.into(), path.into())))
}

#[cfg(unix)]
fn reject_unsafe(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "refusing symbolic-link SSH secret path: {}",
        path.display()
    );
    if directory {
        ensure!(
            metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
            "SSH directory must be private (mode 0700): {}",
            path.display()
        );
    } else {
        ensure!(
            metadata.is_file() && metadata.permissions().mode() & 0o077 == 0,
            "SSH key must be private (mode 0600): {}",
            path.display()
        );
    }
    Ok(())
}

pub fn ensure_key(config: &Path, profile: &str) -> Result<(PathBuf, String)> {
    #[cfg(not(unix))]
    {
        let _ = (config, profile);
        anyhow::bail!("SSH, SCP and ssh-config are supported only on Linux and macOS");
    }
    #[cfg(unix)]
    {
        safe_profile(profile)?;
        let parent = config.parent().unwrap_or(Path::new("."));
        reject_unsafe(parent, true)?;
        let ssh_root = parent.join("ssh");
        if let Err(error) = fs::create_dir(&ssh_root) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        } else {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&ssh_root, fs::Permissions::from_mode(0o700))?;
        }
        reject_unsafe(&ssh_root, true)?;
        let dir = parent.join("ssh").join(profile);
        if !dir.exists() {
            let mut builder = fs::DirBuilder::new();
            use std::os::unix::fs::DirBuilderExt;
            builder.recursive(true).mode(0o700).create(&dir)?;
        }
        reject_unsafe(&dir, true)?;
        let lock_path = dir.join(".key.lock");
        ensure!(
            !lock_path.is_symlink(),
            "refusing symbolic-link SSH lock path"
        );
        let lock = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600))?;
        lock.lock()?;
        let private = dir.join("id_ed25519");
        ensure!(
            !private.is_symlink(),
            "refusing symbolic-link SSH secret path"
        );
        ensure!(
            !private.with_extension("pub").is_symlink(),
            "refusing symbolic-link SSH public key path"
        );
        if !private.exists() {
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
                .arg(&private)
                .status()
                .context("could not run ssh-keygen")?;
            ensure!(status.success(), "ssh-keygen failed with {status}");
        }
        reject_unsafe(&private, false)?;
        let output = Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(&private)
            .output()?;
        ensure!(output.status.success(), "could not read SSH public key");
        let public = String::from_utf8(output.stdout)?;
        let parts: Vec<_> = public.split_whitespace().collect();
        ensure!(
            parts.len() >= 2,
            "ssh-keygen produced an invalid public key"
        );
        let public = format!("{} {}", parts[0], parts[1]);
        ensure!(
            kiln_api::valid_ssh_public_key(&public),
            "ssh-keygen produced a non-Ed25519 key"
        );
        Ok((private, public))
    }
}

pub fn write_known_hosts(config: &Path, profile: &str, alias: &str, key: &str) -> Result<PathBuf> {
    safe_config_value(alias)?;
    ensure!(
        alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid host alias"
    );
    ensure!(kiln_api::valid_ssh_public_key(key), "invalid SSH host key");
    let (private, _) = ensure_key(config, profile)?;
    let path = private
        .parent()
        .unwrap()
        .join(format!("known_hosts_{alias}"));
    ensure!(
        !path.is_symlink(),
        "refusing symbolic-link known_hosts path"
    );
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(private.parent().unwrap())?;
    writeln!(temporary, "{alias} {key}")?;
    temporary.as_file().sync_all()?;
    temporary.persist(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_config_command_injection_characters() {
        for bad in ["bad\nname", "bad%name", "bad'name", "bad\"name"] {
            assert!(safe_config_value(bad).is_err(), "{bad:?}");
        }
        assert!(safe_config_value("path with spaces").is_ok());
    }

    #[test]
    fn proxy_command_quotes_spaces_without_shell_interpolation() {
        let command = proxy_command(Path::new("/tmp/config file"), "work", "kiln").unwrap();
        assert_eq!(
            command,
            "'kiln' '--config' '/tmp/config file' '--profile' 'work' 'ssh-proxy'"
        );
    }

    #[test]
    fn remote_endpoint_requires_full_id_and_absolute_path() {
        let id = "b".repeat(32);
        assert_eq!(
            parse_copy_endpoint(&format!("{id}:/tmp/a")).unwrap(),
            Some((id, "/tmp/a".into()))
        );
        for bad in ["name:/tmp/a", "bbbb:relative", "bbbb:/tmp/a\nHost x"] {
            assert!(parse_copy_endpoint(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_copy_endpoint("local file").unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_private_key() {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let dir = root.path().join("ssh/default");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(dir.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        symlink("/dev/null", dir.join("id_ed25519")).unwrap();
        assert!(
            ensure_key(&root.path().join("profiles.json"), "default")
                .unwrap_err()
                .to_string()
                .contains("symbolic-link")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_ssh_parent_before_key_generation() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        symlink(elsewhere.path(), root.path().join("ssh")).unwrap();
        assert!(
            ensure_key(&root.path().join("profiles.json"), "default")
                .unwrap_err()
                .to_string()
                .contains("symbolic-link")
        );
        assert!(!elsewhere.path().join("default/id_ed25519").exists());
    }
}
