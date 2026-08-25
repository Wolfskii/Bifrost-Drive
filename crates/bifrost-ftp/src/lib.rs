use async_std::io::{ReadExt, WriteExt};
use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt};
use rustls::ClientConfig;
use std::sync::Arc;
use suppaftp::{AsyncFtpStream, AsyncRustlsConnector, AsyncRustlsFtpStream};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpConfig {
    pub endpoint: Url,
    pub username: String,
    pub password: String,
}

pub struct FtpProvider {
    config: FtpConfig,
}

enum Session {
    Plain(AsyncFtpStream),
    Secure(AsyncRustlsFtpStream),
}

impl FtpProvider {
    pub fn connect(config: FtpConfig) -> Result<Self, StorageError> {
        let scheme = config.endpoint.scheme();
        if scheme != "ftp" && scheme != "ftps" {
            return Err(Self::error("endpoint must use ftp:// or ftps://"));
        }
        let Some(host) = config.endpoint.host_str() else {
            return Err(Self::error("FTP endpoint must include a host"));
        };
        if host.trim().is_empty() || config.username.trim().is_empty() {
            return Err(Self::error("FTP host and username are required"));
        }
        Ok(Self { config })
    }

    fn error(message: impl Into<String>) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::Ftp,
            message: message.into(),
        }
    }

    fn endpoint(&self) -> String {
        let host = self.config.endpoint.host_str().unwrap_or_default();
        let port = self.config.endpoint.port_or_known_default().unwrap_or(21);
        format!("{host}:{port}")
    }

    async fn session(&self) -> Result<Session, StorageError> {
        if self.config.endpoint.scheme() == "ftps" {
            let ftp: AsyncRustlsFtpStream = AsyncRustlsFtpStream::connect(self.endpoint())
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let tls = ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let connector =
                AsyncRustlsConnector::from(futures_rustls::TlsConnector::from(Arc::new(tls)));
            let mut ftp = ftp
                .into_secure(
                    connector,
                    self.config.endpoint.host_str().unwrap_or_default(),
                )
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            ftp.login(&self.config.username, &self.config.password)
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            Ok(Session::Secure(ftp))
        } else {
            let mut ftp = AsyncFtpStream::connect(self.endpoint())
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            ftp.login(&self.config.username, &self.config.password)
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            Ok(Session::Plain(ftp))
        }
    }

    fn remote_path(path: &RemotePath) -> &str {
        if path.as_str().is_empty() {
            "."
        } else {
            path.as_str()
        }
    }

    fn metadata(path: RemotePath, entry: &suppaftp::list::File) -> RemoteMetadata {
        let modified = DateTime::<Utc>::from(entry.modified());
        RemoteMetadata {
            path,
            is_directory: entry.is_directory(),
            size_bytes: Some(entry.size() as u64),
            etag: None,
            modified_at: Some(modified),
        }
    }

    async fn write_stream(
        &self,
        path: &RemotePath,
        mut content: bifrost_storage::WriteStream,
    ) -> Result<(), StorageError> {
        match self.session().await? {
            Session::Plain(mut ftp) => {
                let mut data = ftp
                    .put_with_stream(Self::remote_path(path))
                    .await
                    .map_err(|error| Self::error(error.to_string()))?;
                while let Some(chunk) = content.next().await {
                    data.write_all(&chunk.map_err(|error| Self::error(error.to_string()))?)
                        .await
                        .map_err(|error| Self::error(error.to_string()))?;
                }
                ftp.finalize_put_stream(data)
                    .await
                    .map_err(|error| Self::error(error.to_string()))?;
            }
            Session::Secure(mut ftp) => {
                let mut data = ftp
                    .put_with_stream(Self::remote_path(path))
                    .await
                    .map_err(|error| Self::error(error.to_string()))?;
                while let Some(chunk) = content.next().await {
                    data.write_all(&chunk.map_err(|error| Self::error(error.to_string()))?)
                        .await
                        .map_err(|error| Self::error(error.to_string()))?;
                }
                ftp.finalize_put_stream(data)
                    .await
                    .map_err(|error| Self::error(error.to_string()))?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl StorageProvider for FtpProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ftp
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::Rename,
            Capability::CreateDirectory,
        ])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        let mut session = self.session().await?;
        match &mut session {
            Session::Plain(ftp) => ftp
                .feat()
                .await
                .map(|_| ())
                .map_err(|error| Self::error(error.to_string())),
            Session::Secure(ftp) => ftp
                .feat()
                .await
                .map(|_| ())
                .map_err(|error| Self::error(error.to_string())),
        }
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        _cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let mut session = self.session().await?;
        let lines = match &mut session {
            Session::Plain(ftp) => {
                ftp.mlsd((!prefix.as_str().is_empty()).then_some(prefix.as_str()))
                    .await
            }
            Session::Secure(ftp) => {
                ftp.mlsd((!prefix.as_str().is_empty()).then_some(prefix.as_str()))
                    .await
            }
        }
        .map_err(|error| Self::error(error.to_string()))?;
        let entries = lines
            .into_iter()
            .filter_map(|line| suppaftp::list::File::from_mlsx_line(&line).ok())
            .filter_map(|entry| {
                let path = if prefix.as_str().is_empty() {
                    RemotePath::parse(entry.name()).ok()?
                } else {
                    prefix.join(entry.name()).ok()?
                };
                Some(RemoteEntry {
                    metadata: Self::metadata(path, &entry),
                })
            })
            .collect();
        Ok(Page {
            entries,
            next_cursor: None,
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let mut session = self.session().await?;
        let line = match &mut session {
            Session::Plain(ftp) => ftp.mlst(Some(Self::remote_path(path))).await,
            Session::Secure(ftp) => ftp.mlst(Some(Self::remote_path(path))).await,
        }
        .map_err(|error| Self::error(error.to_string()))?;
        let entry = suppaftp::list::File::from_mlsx_line(&line)
            .map_err(|error| Self::error(error.to_string()))?;
        Ok(Self::metadata(path.clone(), &entry))
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let range = request.range;
        let session = self.session().await?;
        let (sender, receiver) = async_channel::bounded::<Result<Bytes, StorageError>>(2);
        let path = request.path;
        async_std::task::spawn(async move {
            let mut source_offset = 0u64;
            match session {
                Session::Plain(mut ftp) => {
                    let mut data = match ftp.retr_as_stream(Self::remote_path(&path)).await {
                        Ok(data) => data,
                        Err(error) => {
                            let _ = sender.send(Err(Self::error(error.to_string()))).await;
                            return;
                        }
                    };
                    loop {
                        let mut buffer = vec![0u8; 64 * 1024];
                        let read = match data.read(&mut buffer).await {
                            Ok(read) => read,
                            Err(error) => {
                                let _ = sender.send(Err(StorageError::Io(error))).await;
                                return;
                            }
                        };
                        if read == 0 {
                            let _ = ftp.finalize_retr_stream(data).await;
                            break;
                        }
                        buffer.truncate(read);
                        if !send_selected(&sender, &buffer, source_offset, range.as_ref()).await {
                            break;
                        }
                        source_offset += read as u64;
                        if range
                            .as_ref()
                            .is_some_and(|requested| source_offset >= requested.end)
                        {
                            break;
                        }
                    }
                }
                Session::Secure(mut ftp) => {
                    let mut data = match ftp.retr_as_stream(Self::remote_path(&path)).await {
                        Ok(data) => data,
                        Err(error) => {
                            let _ = sender.send(Err(Self::error(error.to_string()))).await;
                            return;
                        }
                    };
                    loop {
                        let mut buffer = vec![0u8; 64 * 1024];
                        let read = match data.read(&mut buffer).await {
                            Ok(read) => read,
                            Err(error) => {
                                let _ = sender.send(Err(StorageError::Io(error))).await;
                                return;
                            }
                        };
                        if read == 0 {
                            let _ = ftp.finalize_retr_stream(data).await;
                            break;
                        }
                        buffer.truncate(read);
                        if !send_selected(&sender, &buffer, source_offset, range.as_ref()).await {
                            break;
                        }
                        source_offset += read as u64;
                        if range
                            .as_ref()
                            .is_some_and(|requested| source_offset >= requested.end)
                        {
                            break;
                        }
                    }
                }
            }
        });
        let stream = stream::unfold(receiver, |receiver| async move {
            receiver.recv().await.ok().map(|item| (item, receiver))
        });
        Ok(Box::pin(stream))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let path = request.path;
        self.write_stream(&path, request.content).await?;
        Ok(RemoteMetadata {
            path,
            is_directory: false,
            size_bytes: request.size_bytes,
            etag: None,
            modified_at: request.modified_at,
        })
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let mut session = self.session().await?;
        match &mut session {
            Session::Plain(ftp) => ftp.rm(Self::remote_path(path)).await,
            Session::Secure(ftp) => ftp.rm(Self::remote_path(path)).await,
        }
        .map(|_| ())
        .map_err(|error| Self::error(error.to_string()))
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        let mut session = self.session().await?;
        match &mut session {
            Session::Plain(ftp) => ftp.mkdir(Self::remote_path(path)).await,
            Session::Secure(ftp) => ftp.mkdir(Self::remote_path(path)).await,
        }
        .map(|_| ())
        .map_err(|error| Self::error(error.to_string()))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let mut session = self.session().await?;
        match &mut session {
            Session::Plain(ftp) => {
                ftp.rename(Self::remote_path(from), Self::remote_path(to))
                    .await
            }
            Session::Secure(ftp) => {
                ftp.rename(Self::remote_path(from), Self::remote_path(to))
                    .await
            }
        }
        .map(|_| ())
        .map_err(|error| Self::error(error.to_string()))
    }
}

async fn send_selected(
    sender: &async_channel::Sender<Result<Bytes, StorageError>>,
    bytes: &[u8],
    source_offset: u64,
    range: Option<&std::ops::Range<u64>>,
) -> bool {
    let source_end = source_offset + bytes.len() as u64;
    let selected = match range {
        Some(requested) => {
            if source_end <= requested.start || source_offset >= requested.end {
                return true;
            }
            let from = requested.start.saturating_sub(source_offset) as usize;
            let to = (requested.end.min(source_end) - source_offset) as usize;
            &bytes[from..to]
        }
        None => bytes,
    };
    if selected.is_empty() {
        return true;
    }
    sender
        .send(Ok(Bytes::copy_from_slice(selected)))
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{FtpConfig, FtpProvider};
    use url::Url;

    #[test]
    fn accepts_only_ftp_schemes() {
        assert!(FtpProvider::connect(FtpConfig {
            endpoint: Url::parse("https://example.test").unwrap(),
            username: "user".to_owned(),
            password: "password".to_owned(),
        })
        .is_err());
    }

    #[test]
    fn accepts_explicit_ftps_configuration() {
        assert!(FtpProvider::connect(FtpConfig {
            endpoint: Url::parse("ftps://example.test").unwrap(),
            username: "user".to_owned(),
            password: "password".to_owned(),
        })
        .is_ok());
    }
}
