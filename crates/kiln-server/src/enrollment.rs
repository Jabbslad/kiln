//! Explicit local administrative enrollment. Never invoked by the installer.
use crate::auth::Enrollment;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    future::Future,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const ETC: &str = "/etc/kiln";
const DROPIN: &str = "/etc/systemd/system/kiln-api.service.d";
const STATE_UNIT: &[u8] = b"[Service]\nStateDirectory=kiln-api\nStateDirectoryMode=0700\n";

#[derive(clap::Args)]
pub struct Options {
    /// Override the identity service (default: https://dark-forge.dev).
    #[arg(long)]
    issuer: Option<String>,
    #[arg(long, required_unless_present = "resume")]
    url: Option<String>,
    #[arg(long, default_value = "/etc/kiln/ca.crt")]
    ca_file: PathBuf,
    #[arg(long, default_value = "kiln")]
    name: String,
    /// Resume saved enrollment after readiness or activation failed.
    #[arg(long, conflicts_with_all = ["issuer", "url"])]
    resume: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[doc(hidden)]
pub struct Receipt {
    pub enrollment: Enrollment,
    pub origin: String,
    pub ca_pem: String,
    pub directory_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    registration_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

fn origin(value: &str) -> Result<reqwest::Url> {
    let u = reqwest::Url::parse(value)?;
    ensure!(
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "expected HTTPS origin"
    );
    Ok(u)
}
fn directory(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0,
        "unsafe enrollment directory"
    );
    Ok(())
}
fn read(path: &Path, mask: u32, limit: u64) -> Result<Vec<u8>> {
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let m = f.metadata()?;
    ensure!(
        m.is_file()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & mask == 0
            && m.len() <= limit,
        "unsafe enrollment file"
    );
    let mut bytes = Vec::new();
    f.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "enrollment file too large");
    Ok(bytes)
}
fn install_file(path: &Path, bytes: &[u8], mode: u32, gid: u32) -> Result<()> {
    let parent = path.parent().context("missing directory")?;
    directory(parent)?;
    if fs::symlink_metadata(path).is_ok() {
        let existing = read(path, 0o022, 131072)?;
        let m = fs::symlink_metadata(path)?;
        ensure!(
            existing == bytes && m.mode() & 0o777 == mode && m.gid() == gid,
            "existing enrollment differs; refusing replacement"
        );
        return Ok(());
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    use std::os::fd::AsRawFd;
    ensure!(
        unsafe { libc::fchown(temporary.as_file().as_raw_fd(), libc::uid_t::MAX, gid) } == 0,
        "could not set enrollment group"
    );
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
async fn json<T: serde::de::DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 131072,
            "identity response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
async fn systemctl(args: &[&str]) -> Result<()> {
    let status = tokio::process::Command::new("/usr/bin/systemctl")
        .args(args)
        .status()
        .await?;
    ensure!(
        status.success(),
        "systemd action failed; retain enrollment and use --resume"
    );
    Ok(())
}

/// Internal boundary used to exercise the post-approval transaction without
/// touching the host's systemd or network services.
#[doc(hidden)]
pub trait EnrollmentEffects {
    fn systemctl(&mut self, args: &[&str]) -> impl Future<Output = Result<()>>;
    fn readiness(&mut self) -> impl Future<Output = Result<()>>;
    fn activate(&mut self, directory_token: &str) -> impl Future<Output = Result<()>>;
}

#[doc(hidden)]
pub struct EnrollmentPaths<'a> {
    pub etc: &'a Path,
    pub dropin: &'a Path,
}

#[doc(hidden)]
pub async fn complete<E: EnrollmentEffects>(
    receipt: &Receipt,
    paths: EnrollmentPaths<'_>,
    service_gid: u32,
    owner_gid: u32,
    effects: &mut E,
) -> Result<()> {
    let receipt_path = paths.etc.join("enrollment.pending.json");
    install_file(
        &paths.etc.join("identity.json"),
        &serde_json::to_vec(&receipt.enrollment)?,
        0o640,
        service_gid,
    )?;
    install_file(
        &paths.etc.join("directory.token"),
        receipt.directory_token.as_bytes(),
        0o600,
        owner_gid,
    )?;
    install_file(
        &paths.dropin.join("identity.conf"),
        STATE_UNIT,
        0o644,
        owner_gid,
    )?;
    effects.systemctl(&["daemon-reload"]).await?;
    effects.systemctl(&["restart", "kiln-api.service"]).await?;
    effects.readiness().await?;
    effects.activate(&receipt.directory_token).await?;
    fs::remove_file(receipt_path)?;
    fs::File::open(paths.etc)?.sync_all()?;
    Ok(())
}

struct ProductionEffects<'a> {
    receipt: &'a Receipt,
    http: &'a reqwest::Client,
    direct: reqwest::Client,
    admin: String,
}

impl EnrollmentEffects for ProductionEffects<'_> {
    async fn systemctl(&mut self, args: &[&str]) -> Result<()> {
        systemctl(args).await
    }

    async fn readiness(&mut self) -> Result<()> {
        for _ in 0..30 {
            if let Ok(response) = self
                .direct
                .get(origin(&self.receipt.origin)?.join("v1/templates")?)
                .bearer_auth(&self.admin)
                .send()
                .await
                && response.status().is_success()
                && json::<Vec<kiln_api::TemplateView>>(response).await.is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!("gateway readiness failed; retain state and run `kiln-api enroll --resume`")
    }

    async fn activate(&mut self, directory_token: &str) -> Result<()> {
        self.http
            .post(origin(&self.receipt.enrollment.issuer)?.join("v1/registrations/activate")?)
            .bearer_auth(directory_token)
            .send()
            .await?
            .error_for_status()
            .context("activation not confirmed; run `kiln-api enroll --resume`")?;
        Ok(())
    }
}

pub async fn run(options: Options) -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "server enrollment requires root"
    );
    directory(Path::new(ETC))?;
    directory(Path::new(DROPIN))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open("/run/lock/kiln-install.lock")?;
    let m = lock.metadata()?;
    ensure!(
        m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0,
        "unsafe installer lock"
    );
    lock.try_lock()
        .context("another installer or enrollment is running")?;
    let receipt_path = Path::new(ETC).join("enrollment.pending.json");
    let http = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()?;
    let receipt: Receipt = if options.resume {
        serde_json::from_slice(&read(&receipt_path, 0o077, 131072)?)?
    } else {
        ensure!(
            fs::symlink_metadata(Path::new(ETC).join("identity.json")).is_err(),
            "server already enrolled; refusing owner replacement"
        );
        ensure!(
            fs::symlink_metadata(&receipt_path).is_err(),
            "enrollment is pending; use --resume"
        );
        let issuer = options
            .issuer
            .unwrap_or_else(|| kiln_api::auth::DEFAULT_IDENTITY_ORIGIN.to_owned());
        let url = options.url.context("server URL required")?;
        let central = origin(&issuer)?;
        let issuer = central.origin().ascii_serialization();
        origin(&url)?;
        let ca = String::from_utf8(read(&options.ca_file, 0o022, 65536)?)?;
        reqwest::Certificate::from_pem(ca.as_bytes())?;
        let (rest, certificate) = x509_parser::pem::parse_x509_pem(ca.as_bytes())
            .map_err(|_| anyhow::anyhow!("invalid CA certificate"))?;
        ensure!(
            rest.iter().all(u8::is_ascii_whitespace),
            "only one CA certificate is allowed"
        );
        let fingerprint = Sha256::digest(&certificate.contents)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        ensure!(
            !options.name.is_empty() && options.name.len() <= 128,
            "invalid server name"
        );
        let pending: Pending = json(
            http.post(central.join("v1/registrations")?)
                .json(&serde_json::json!({"name":options.name,"origin":url,"ca_pem":ca}))
                .send()
                .await?
                .error_for_status()?,
        )
        .await?;
        ensure!(
            (1..=600).contains(&pending.expires_in) && (1..=60).contains(&pending.interval),
            "invalid registration lifetime"
        );
        let approval = reqwest::Url::parse(&pending.verification_uri)?;
        ensure!(
            approval.origin() == central.origin()
                && approval.username().is_empty()
                && approval.password().is_none(),
            "invalid approval origin"
        );
        eprintln!(
            "Approve enrollment at {} with code {}.\nServer: {}\nCA SHA-256: {}\nOnly approve the request you started on this host.",
            pending.verification_uri, pending.user_code, url, fingerprint
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(pending.expires_in);
        let mut interval = pending.interval;
        let registration: kiln_api::auth::Registration = loop {
            tokio::select! {
                _=tokio::signal::ctrl_c()=>bail!("enrollment cancelled"),
                _=tokio::time::sleep_until(std::cmp::min(deadline,tokio::time::Instant::now()+Duration::from_secs(interval)))=>{},
            }
            ensure!(tokio::time::Instant::now() < deadline, "enrollment expired");
            // No retries: a lost final response requires fresh approval, not replay.
            let response = http
                .post(central.join("v1/registrations/poll")?)
                .json(&serde_json::json!({"registration_code":pending.registration_code}))
                .send()
                .await?;
            let status = response.status();
            let value: serde_json::Value = json(response).await?;
            if value["error"] == "slow_down" {
                interval = interval.saturating_add(5);
                continue;
            }
            ensure!(
                status.is_success(),
                "registration failed; start a fresh enrollment"
            );
            if value["status"] == "pending" {
                continue;
            }
            ensure!(value["status"] != "denied", "enrollment denied");
            break serde_json::from_value(value)?;
        };
        let receipt = Receipt {
            enrollment: Enrollment {
                version: 1,
                issuer,
                owner_id: registration.owner_id,
                server_id: registration.server_id,
            },
            origin: url,
            ca_pem: ca,
            directory_token: registration.directory_token,
        };
        receipt.enrollment.validate()?;
        install_file(&receipt_path, &serde_json::to_vec(&receipt)?, 0o600, 0)?;
        receipt
    };
    receipt.enrollment.validate()?;
    origin(&receipt.origin)?;
    let group = unsafe { libc::getgrnam(c"kiln-api".as_ptr()) };
    ensure!(!group.is_null(), "kiln-api group is missing");
    let gid = unsafe { (*group).gr_gid };
    eprintln!(
        "Enrollment saved. Restarting only kiln-api; existing API/SSH connections will disconnect."
    );
    let admin = kiln_api::read_token(&Path::new(ETC).join("admin.token"))?;
    let direct = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(2))
        .add_root_certificate(reqwest::Certificate::from_pem(receipt.ca_pem.as_bytes())?)
        .build()?;
    let mut effects = ProductionEffects {
        receipt: &receipt,
        http: &http,
        direct,
        admin,
    };
    complete(
        &receipt,
        EnrollmentPaths {
            etc: Path::new(ETC),
            dropin: Path::new(DROPIN),
        },
        gid,
        0,
        &mut effects,
    )
    .await?;
    eprintln!("Server enrolled. Log in on your laptop with `kiln login`.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_issuer_does_not_require_an_argument_or_conflict_with_resume() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            options: Options,
        }
        let fresh = Cli::try_parse_from(["enroll", "--url", "https://host.example:8443"])
            .expect("the public issuer must not require an argument");
        assert!(fresh.options.issuer.is_none());
        let resume = Cli::try_parse_from(["enroll", "--resume"]).unwrap();
        assert!(resume.options.resume);
        assert!(
            Cli::try_parse_from(["enroll", "--resume", "--issuer", "https://other.test"]).is_err()
        );
    }

    #[test]
    fn persistence_is_private_idempotent_and_refuses_symlinks_or_replacement() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let p = dir.path().join("receipt");
        let gid = unsafe { libc::getegid() };
        install_file(&p, b"receipt", 0o600, gid).unwrap();
        install_file(&p, b"receipt", 0o600, gid).unwrap();
        assert!(install_file(&p, b"replacement", 0o600, gid).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"receipt");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(install_file(&link, b"receipt", 0o600, gid).is_err());
        assert_eq!(fs::metadata(p).unwrap().mode() & 0o777, 0o600);
    }
}
