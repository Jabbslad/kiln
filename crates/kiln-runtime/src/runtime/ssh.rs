use super::*;
use kiln_protocol::{SshReady, valid_ssh_public_key};
use tokio::net::UnixStream;

impl Runtime {
    pub async fn ssh_host_key(&self, id: &str) -> Result<SshReady> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if record.state != "running" {
            return Err(Error::Invalid("SSH requires a running box".into()));
        }
        match self
            .guest(&record, &Request::SshHostKey, Duration::from_secs(10))
            .await?
        {
            Response::SshHostKey { public_key } if valid_ssh_public_key(&public_key) => {
                Ok(SshReady { public_key })
            }
            _ => Err(Error::Invalid("guest image does not support SSH".into())),
        }
    }

    pub async fn ssh_connect(&self, id: &str, public_key: &str) -> Result<UnixStream> {
        let _lock = storage::lock(&self.directory(id)?)?;
        let mut record = self.read(id)?;
        self.observed(&mut record).await?;
        if record.state != "running" {
            return Err(Error::Invalid("SSH requires a running box".into()));
        }
        let directory = File::open(self.run(&record)?)?;
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}/vsock.sock",
            directory.as_raw_fd()
        ));
        let (stream, _) = guest::ssh(&path, public_key).await?;
        // Release the lifecycle lock after connecting to this VMM generation.
        // Stopping the VMM closes the stream; it can never follow a replacement.
        Ok(stream)
    }
}
