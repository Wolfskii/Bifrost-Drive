use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageCapacity, StorageError, StorageProvider,
    WriteRequest,
};
use bytes::Bytes;
use chrono::{DateTime, TimeZone, Utc};
use futures_util::{stream, StreamExt};
use russh::{
    cipher, client,
    keys::{
        check_known_hosts_path, known_hosts::learn_known_hosts_path, load_secret_key,
        PrivateKeyWithHashAlg, PublicKeyOrCertificate,
    },
};
use russh_sftp::{
    client::{error::Error as SftpError, SftpSession},
    protocol::{OpenFlags, StatusCode},
};
use std::borrow::Cow;
use std::{
    ops::Range,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;

const READ_PIPELINE_CHUNK_SIZE: u64 = 1024 * 1024;
const READ_PIPELINE_CONCURRENCY: usize = 16;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEPALIVE_MAX_MISSED: usize = 3;
const SESSION_REVALIDATE_AFTER: Duration = Duration::from_secs(60);
const SESSION_VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub known_hosts: PathBuf,
    pub root_path: String,
    pub trust_on_first_use: bool,
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
    session: Mutex<Option<CachedSession>>,
}

struct CachedSession {
    value: Arc<SftpSession>,
    last_used: Instant,
}

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    trust_on_first_use: bool,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public_key = server_public_key.public_key();
        if check_known_hosts_path(&self.host, self.port, &public_key, &self.known_hosts)? {
            return Ok(true);
        }
        if self.trust_on_first_use {
            learn_known_hosts_path(&self.host, self.port, &public_key, &self.known_hosts)?;
            return Ok(true);
        }
        Ok(false)
    }
}

impl SftpProvider {
    pub fn connect(config: SftpConfig, password: impl Into<String>) -> Result<Self, StorageError> {
        let mut config = config;
        config.root_path = Self::normalize_root_path(&config.root_path)?;
        if config.host.trim().is_empty() || config.username.trim().is_empty() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: "SFTP host and username are required".to_owned(),
            });
        }
        if !config.trust_on_first_use && !config.known_hosts.is_file() {
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
            session: Mutex::new(None),
        })
    }

    pub fn connect_with_private_key(
        config: SftpConfig,
        key_path: PathBuf,
        passphrase: Option<String>,
    ) -> Result<Self, StorageError> {
        let mut config = config;
        config.root_path = Self::normalize_root_path(&config.root_path)?;
        if config.host.trim().is_empty() || config.username.trim().is_empty() {
            return Err(StorageError::Provider {
                provider: ProviderKind::Sftp,
                message: "SFTP host and username are required".to_owned(),
            });
        }
        if !config.trust_on_first_use && !config.known_hosts.is_file() {
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
            session: Mutex::new(None),
        })
    }

    async fn connect_session(&self) -> Result<SftpSession, StorageError> {
        let ssh_config = client::Config {
            window_size: 64 * 1024 * 1024,
            maximum_packet_size: 256 * 1024,
            channel_buffer_size: 1024,
            keepalive_interval: Some(KEEPALIVE_INTERVAL),
            keepalive_max: KEEPALIVE_MAX_MISSED,
            nodelay: true,
            ..Default::default()
        };
        let mut ssh_config = ssh_config;
        let mut ciphers = ssh_config.preferred.cipher.to_vec();
        ciphers.sort_by_key(|name| {
            if *name == cipher::AES_128_GCM {
                0
            } else if *name == cipher::AES_256_GCM {
                1
            } else if *name == cipher::CHACHA20_POLY1305 {
                2
            } else {
                3
            }
        });
        ssh_config.preferred.cipher = Cow::Owned(ciphers);
        let handler = ClientHandler {
            host: self.config.host.clone(),
            port: self.config.port,
            known_hosts: self.config.known_hosts.clone(),
            trust_on_first_use: self.config.trust_on_first_use,
        };
        let mut session = client::connect(
            Arc::new(ssh_config),
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

    async fn session(&self) -> Result<Arc<SftpSession>, StorageError> {
        let mut cached = self.session.lock().await;
        if let Some(session) = cached.as_mut() {
            if session.last_used.elapsed() < SESSION_REVALIDATE_AFTER {
                session.last_used = Instant::now();
                return Ok(Arc::clone(&session.value));
            }
            let root = self.path(&RemotePath::root());
            if matches!(
                tokio::time::timeout(SESSION_VALIDATION_TIMEOUT, session.value.canonicalize(root))
                    .await,
                Ok(Ok(_))
            ) {
                session.last_used = Instant::now();
                return Ok(Arc::clone(&session.value));
            }
            *cached = None;
        }
        let session = Arc::new(self.connect_session().await?);
        *cached = Some(CachedSession {
            value: Arc::clone(&session),
            last_used: Instant::now(),
        });
        Ok(session)
    }

    fn normalize_root_path(value: &str) -> Result<String, StorageError> {
        let normalized = value.trim().replace('\\', "/");
        let absolute = normalized.starts_with('/');
        let mut components = Vec::new();
        for component in normalized.split('/') {
            match component {
                "" | "." => continue,
                ".." => {
                    return Err(Self::map_error("SFTP start path cannot contain '..'"));
                }
                component => components.push(component),
            }
        }
        let path = components.join("/");
        Ok(if absolute { format!("/{path}") } else { path })
    }

    fn path(&self, path: &RemotePath) -> String {
        if self.config.root_path.is_empty() {
            return if path.as_str().is_empty() {
                ".".to_owned()
            } else {
                path.as_str().to_owned()
            };
        }
        if path.as_str().is_empty() {
            return self.config.root_path.clone();
        }
        if self.config.root_path == "/" {
            format!("/{}", path.as_str())
        } else {
            format!("{}/{}", self.config.root_path, path.as_str())
        }
    }

    fn map_error(error: impl std::fmt::Display) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::Sftp,
            message: error.to_string(),
        }
    }

    fn map_path_error(error: SftpError, path: &RemotePath) -> StorageError {
        match error {
            SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile => {
                StorageError::NotFound { path: path.clone() }
            }
            SftpError::Status(status) if status.status_code == StatusCode::PermissionDenied => {
                StorageError::PermissionDenied { path: path.clone() }
            }
            error => Self::map_error(error),
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
            .canonicalize(self.path(&RemotePath::root()))
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
            .read_dir(self.path(prefix))
            .await
            .map_err(|error| Self::map_path_error(error, prefix))?;
        let entries = directory
            .map(|entry| {
                let path = prefix
                    .join(&entry.file_name())
                    .unwrap_or_else(|_| RemotePath::root());
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
            .metadata(self.path(path))
            .await
            .map(|metadata| Self::map_metadata(path.clone(), metadata))
            .map_err(|error| Self::map_path_error(error, path))
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let session = self.session().await?;
        let (start, limit) = Self::range(request.range)?;
        if let Some(limit) = limit {
            let path = self.path(&request.path);
            let remote_path = request.path;
            let chunk_count = limit.div_ceil(READ_PIPELINE_CHUNK_SIZE);
            let chunks = (0..chunk_count).map(move |index| {
                let session = Arc::clone(&session);
                let path = path.clone();
                let remote_path = remote_path.clone();
                let offset = start + index * READ_PIPELINE_CHUNK_SIZE;
                let length = (limit - index * READ_PIPELINE_CHUNK_SIZE)
                    .min(READ_PIPELINE_CHUNK_SIZE) as usize;
                async move {
                    let mut file = session
                        .open(path)
                        .await
                        .map_err(|error| Self::map_path_error(error, &remote_path))?;
                    file.seek(std::io::SeekFrom::Start(offset))
                        .await
                        .map_err(StorageError::Io)?;
                    let mut buffer = vec![0; length];
                    let mut read = 0;
                    while read < length {
                        let size = file
                            .read(&mut buffer[read..])
                            .await
                            .map_err(StorageError::Io)?;
                        if size == 0 {
                            break;
                        }
                        read += size;
                    }
                    if read < length && index + 1 < chunk_count {
                        return Err(StorageError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            format!(
                                "SFTP range chunk at offset {offset} ended after {read} of {length} bytes"
                            ),
                        )));
                    }
                    buffer.truncate(read);
                    Ok(Bytes::from(buffer))
                }
            });
            return Ok(Box::pin(
                stream::iter(chunks).buffered(READ_PIPELINE_CONCURRENCY),
            ));
        }
        let mut file = session
            .open(self.path(&request.path))
            .await
            .map_err(|error| Self::map_path_error(error, &request.path))?;
        if start > 0 {
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(Self::map_error)?;
        }
        let stream = stream::try_unfold(file, |mut file| async move {
            let chunk_size = READ_PIPELINE_CHUNK_SIZE as usize;
            let mut buffer = vec![0; chunk_size];
            let size = file.read(&mut buffer).await.map_err(StorageError::Io)?;
            if size == 0 {
                return Ok(None);
            }
            buffer.truncate(size);
            Ok(Some((Bytes::from(buffer), file)))
        });
        Ok(Box::pin(stream))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let session = self.session().await?;
        let path = request.path;
        let mut file = session
            .open_with_flags(
                self.path(&path),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|error| Self::map_path_error(error, &path))?;
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
            .metadata(self.path(path))
            .await
            .map_err(|error| Self::map_path_error(error, path))?;
        if metadata.is_dir() {
            session.remove_dir(self.path(path)).await
        } else {
            session.remove_file(self.path(path)).await
        }
        .map_err(|error| Self::map_path_error(error, path))
    }

    async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
        let session = self.session().await?;
        let Some(info) = session
            .fs_info(self.path(&RemotePath::root()))
            .await
            .map_err(Self::map_error)?
        else {
            return Ok(None);
        };
        Ok(Some(StorageCapacity {
            total_bytes: info.blocks.saturating_mul(info.fragment_size),
            available_bytes: info.blocks_avail.saturating_mul(info.fragment_size),
        }))
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        self.session()
            .await?
            .create_dir(self.path(path))
            .await
            .map_err(|error| Self::map_path_error(error, path))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        self.session()
            .await?
            .rename(self.path(from), self.path(to))
            .await
            .map_err(|error| Self::map_path_error(error, from))
    }
}

#[cfg(test)]
mod tests {
    use super::{SftpConfig, SftpProvider};
    use bifrost_common::RemotePath;
    use bifrost_storage::StorageError;
    use russh_sftp::{
        client::error::Error as SftpError,
        protocol::{Status, StatusCode},
    };
    use std::path::PathBuf;

    #[test]
    fn maps_path_statuses_for_filesystem_decisions() {
        let path = RemotePath::parse("missing.txt").unwrap();
        let status = |status_code| {
            SftpError::Status(Status {
                id: 1,
                status_code,
                error_message: String::new(),
                language_tag: String::new(),
            })
        };

        assert!(matches!(
            SftpProvider::map_path_error(status(StatusCode::NoSuchFile), &path),
            StorageError::NotFound { .. }
        ));
        assert!(matches!(
            SftpProvider::map_path_error(status(StatusCode::PermissionDenied), &path),
            StorageError::PermissionDenied { .. }
        ));
    }

    #[test]
    fn refuses_missing_known_hosts_instead_of_trusting_unknown_servers() {
        let result = SftpProvider::connect(
            SftpConfig {
                host: "example.test".to_owned(),
                port: 22,
                username: "user".to_owned(),
                known_hosts: PathBuf::from("missing-known-hosts"),
                root_path: String::new(),
                trust_on_first_use: false,
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
                root_path: String::new(),
                trust_on_first_use: false,
            },
            directory.path().join("missing-key"),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn allows_missing_known_hosts_for_explicit_first_use_trust() {
        let result = SftpProvider::connect(
            SftpConfig {
                host: "example.test".to_owned(),
                port: 22,
                username: "user".to_owned(),
                known_hosts: PathBuf::from("missing-known-hosts"),
                root_path: String::new(),
                trust_on_first_use: true,
            },
            "secret",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn accepts_absolute_start_paths_without_parent_traversal() {
        assert_eq!(SftpProvider::normalize_root_path("/data").unwrap(), "/data");
        assert!(SftpProvider::normalize_root_path("/data/../etc").is_err());
    }
}
