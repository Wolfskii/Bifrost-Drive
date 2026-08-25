use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest,
};
use bytes::Bytes;
use chrono::{DateTime, TimeZone, Utc};
use futures_util::{stream, StreamExt};
use russh::{
    client,
    keys::{
        check_known_hosts_path, load_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
    },
};
use russh_sftp::{client::SftpSession, protocol::OpenFlags};
use std::{ops::Range, path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub known_hosts: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpAuthentication {
    Password(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
}

pub struct SftpProvider {
    config: SftpConfig,
    authentication: SftpAuthentication,
}

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(check_known_hosts_path(
            &self.host,
            self.port,
            &server_public_key.public_key(),
            &self.known_hosts,
        )?)
    }
}

impl SftpProvider {
    pub fn connect(config: SftpConfig, password: impl Into<String>) -> Result<Self, StorageError> {
        if config.host.trim().is_empty() || config.username.trim().is_empty() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: "SFTP host and username are required".to_owned(),
            });
        }
        if !config.known_hosts.is_file() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: format!(
                    "known_hosts file does not exist: {}",
                    config.known_hosts.display()
                ),
            });
        }
        Ok(Self {
            config,
            authentication: SftpAuthentication::Password(password.into()),
        })
    }

    pub fn connect_with_private_key(
        config: SftpConfig,
        key_path: PathBuf,
        passphrase: Option<String>,
    ) -> Result<Self, StorageError> {
        if config.host.trim().is_empty() || config.username.trim().is_empty() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: "SFTP host and username are required".to_owned(),
            });
        }
        if !config.known_hosts.is_file() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: format!(
                    "known_hosts file does not exist: {}",
                    config.known_hosts.display()
                ),
            });
        }
        if !key_path.is_file() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: format!("private key file does not exist: {}", key_path.display()),
            });
        }
        Ok(Self {
            config,
            authentication: SftpAuthentication::PrivateKey {
                path: key_path,
                passphrase,
            },
        })
    }

    async fn session(&self) -> Result<SftpSession, StorageError> {
        let handler = ClientHandler {
            host: self.config.host.clone(),
            port: self.config.port,
            known_hosts: self.config.known_hosts.clone(),
        };
        let mut session = client::connect(
            Arc::new(client::Config::default()),
            (self.config.host.as_str(), self.config.port),
            handler,
        )
        .await
        .map_err(|error| StorageError::Network {
            provider: ProviderKind::Sftp,
            message: error.to_string(),
        })?;
        let authentication = match &self.authentication {
            SftpAuthentication::Password(password) => session
                .authenticate_password(&self.config.username, password)
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::Sftp,
                    message: error.to_string(),
                })?,
            SftpAuthentication::PrivateKey { path, passphrase } => {
                let path = path.clone();
                let passphrase = passphrase.clone();
                let key = tokio::task::spawn_blocking(move || {
                    load_secret_key(path, passphrase.as_deref())
                })
                .await
                .map_err(|error| StorageError::Provider {
                    provider: ProviderKind::Sftp,
                    message: error.to_string(),
                })?
                .map_err(Self::map_error)?;
                let key = PrivateKeyWithHashAlg::new(
                    Arc::new(key),
                    session
                        .best_supported_rsa_hash()
                        .await
                        .map_err(Self::map_error)?
                        .flatten(),
                );
                session
                    .authenticate_publickey(&self.config.username, key)
                    .await
                    .map_err(|error| StorageError::Network {
                        provider: ProviderKind::Sftp,
                        message: error.to_string(),
                    })?
            }
        };
        if !authentication.success() {
            return Err(StorageError::AuthenticationFailed {
                provider: ProviderKind::Sftp,
            });
        }
        let channel =
            session
                .channel_open_session()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::Sftp,
                    message: error.to_string(),
                })?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| StorageError::Network {
                provider: ProviderKind::Sftp,
                message: error.to_string(),
            })?;
        SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| StorageError::Network {
                provider: ProviderKind::Sftp,
                message: error.to_string(),
            })
    }

    fn path(path: &RemotePath) -> String {
        path.as_str().to_owned()
    }

    fn map_error(error: impl std::fmt::Display) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::Sftp,
            message: error.to_string(),
        }
    }

    fn map_time(value: Option<std::time::SystemTime>) -> Option<DateTime<Utc>> {
        value
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|duration| {
                Utc.timestamp_opt(duration.as_secs() as i64, duration.subsec_nanos())
                    .single()
            })
    }

    fn map_metadata(
        path: RemotePath,
        metadata: russh_sftp::client::fs::Metadata,
    ) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: metadata.is_dir(),
            size_bytes: Some(metadata.len()),
            etag: None,
            modified_at: Self::map_time(metadata.modified().ok()),
        }
    }

    fn range(range: Option<Range<u64>>) -> Result<(u64, Option<u64>), StorageError> {
        let Some(range) = range else {
            return Ok((0, None));
        };
        if range.start >= range.end {
            return Err(Self::map_error("read range must have a positive length"));
        }
        Ok((range.start, Some(range.end - range.start)))
    }

    pub fn uses_private_key(&self) -> bool {
        matches!(self.authentication, SftpAuthentication::PrivateKey { .. })
    }
}

#[async_trait]
impl StorageProvider for SftpProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Sftp
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::Rename,
            Capability::CreateDirectory,
            Capability::RangeRead,
        ])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        let session = self.session().await?;
        session
            .canonicalize(".")
            .await
            .map(|_| ())
            .map_err(Self::map_error)
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        _cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let session = self.session().await?;
        let directory = session
            .read_dir(Self::path(prefix))
            .await
            .map_err(Self::map_error)?;
        let entries = directory
            .map(|entry| {
                let path = RemotePath::parse(&entry.path()).unwrap_or_else(|_| RemotePath::root());
                RemoteEntry {
                    metadata: Self::map_metadata(path, entry.metadata()),
                }
            })
            .collect();
        Ok(Page {
            entries,
            next_cursor: None,
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let session = self.session().await?;
        session
            .metadata(Self::path(path))
            .await
            .map(|metadata| Self::map_metadata(path.clone(), metadata))
            .map_err(Self::map_error)
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let session = self.session().await?;
        let (start, limit) = Self::range(request.range)?;
        let mut file = session
            .open(Self::path(&request.path))
            .await
            .map_err(Self::map_error)?;
        if start > 0 {
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(Self::map_error)?;
        }
        let stream =
            stream::try_unfold((file, 0u64, limit), |(mut file, read, limit)| async move {
                if limit.is_some_and(|limit| read >= limit) {
                    return Ok(None);
                }
                let chunk_size = limit
                    .map(|limit| (limit - read).min(64 * 1024) as usize)
                    .unwrap_or(64 * 1024);
                let mut buffer = vec![0; chunk_size];
                let size = file.read(&mut buffer).await.map_err(StorageError::Io)?;
                if size == 0 {
                    return Ok(None);
                }
                buffer.truncate(size);
                Ok(Some((
                    Bytes::from(buffer),
                    (file, read + size as u64, limit),
                )))
            });
        Ok(Box::pin(stream))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let session = self.session().await?;
        let path = request.path;
        let mut file = session
            .open_with_flags(
                Self::path(&path),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(Self::map_error)?;
        let mut content = request.content;
        while let Some(chunk) = content.next().await {
            file.write_all(&chunk.map_err(Self::map_error)?)
                .await
                .map_err(StorageError::Io)?;
        }
        file.shutdown().await.map_err(StorageError::Io)?;
        Ok(RemoteMetadata {
            path,
            is_directory: false,
            size_bytes: request.size_bytes,
            etag: None,
            modified_at: request.modified_at,
        })
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let session = self.session().await?;
        let metadata = session
            .metadata(Self::path(path))
            .await
            .map_err(Self::map_error)?;
        if metadata.is_dir() {
            session.remove_dir(Self::path(path)).await
        } else {
            session.remove_file(Self::path(path)).await
        }
        .map_err(Self::map_error)
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        self.session()
            .await?
            .create_dir(Self::path(path))
            .await
            .map_err(Self::map_error)
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        self.session()
            .await?
            .rename(Self::path(from), Self::path(to))
            .await
            .map_err(Self::map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::{SftpConfig, SftpProvider};
    use std::path::PathBuf;

    #[test]
    fn refuses_missing_known_hosts_instead_of_trusting_unknown_servers() {
        let result = SftpProvider::connect(
            SftpConfig {
                host: "example.test".to_owned(),
                port: 22,
                username: "user".to_owned(),
                known_hosts: PathBuf::from("missing-known-hosts"),
            },
            "secret",
        );
        assert!(result.is_err());
    }

    #[test]
    fn refuses_missing_private_keys_before_network_access() {
        let directory = tempfile::tempdir().unwrap();
        let known_hosts = directory.path().join("known_hosts");
        std::fs::write(&known_hosts, "").unwrap();
        let result = SftpProvider::connect_with_private_key(
            SftpConfig {
                host: "example.test".to_owned(),
                port: 22,
                username: "user".to_owned(),
                known_hosts,
            },
            directory.path().join("missing-key"),
            None,
        );
        assert!(result.is_err());
    }
}
