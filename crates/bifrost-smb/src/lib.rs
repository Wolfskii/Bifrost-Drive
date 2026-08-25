use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt};
use smb2::{ClientConfig, DirectoryEntry, FileInfo, SmbClient, Tree};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbConfig {
    pub endpoint: Url,
    pub username: String,
    pub password: String,
    pub domain: String,
}

pub struct SmbProvider {
    session: Arc<Mutex<SmbSession>>,
}

struct SmbSession {
    client: SmbClient,
    tree: Tree,
}

impl SmbProvider {
    pub async fn connect(config: SmbConfig) -> Result<Self, StorageError> {
        if config.endpoint.scheme() != "smb" {
            return Err(Self::error("endpoint must use smb://"));
        }
        let host = config
            .endpoint
            .host_str()
            .ok_or_else(|| Self::error("SMB endpoint must include a host"))?;
        let share = config
            .endpoint
            .path_segments()
            .and_then(|mut segments| segments.next())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Self::error("SMB endpoint must include a share name"))?;
        let addr = match config.endpoint.port() {
            Some(port) => format!("{host}:{port}"),
            None => format!("{host}:445"),
        };
        let client = SmbClient::connect(ClientConfig {
            addr,
            timeout: Duration::from_secs(15),
            username: config.username,
            password: config.password,
            domain: config.domain,
            auto_reconnect: true,
            compression: true,
            dfs_enabled: true,
            dfs_target_overrides: Default::default(),
        })
        .await
        .map_err(|error| Self::error(error.to_string()))?;
        let mut client = client;
        let tree = client
            .connect_share(share)
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        Ok(Self {
            session: Arc::new(Mutex::new(SmbSession { client, tree })),
        })
    }

    fn error(message: impl Into<String>) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::Smb,
            message: message.into(),
        }
    }

    fn path(path: &RemotePath) -> String {
        if path.as_str().is_empty() {
            String::new()
        } else {
            path.as_str().replace('/', "\\")
        }
    }

    fn metadata(path: RemotePath, info: &FileInfo) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: info.is_directory,
            size_bytes: Some(info.size),
            etag: None,
            modified_at: info.modified.to_system_time().map(DateTime::<Utc>::from),
        }
    }

    fn entry_metadata(prefix: &RemotePath, entry: &DirectoryEntry) -> Option<RemoteEntry> {
        let path = if prefix.as_str().is_empty() {
            RemotePath::parse(&entry.name).ok()?
        } else {
            prefix.join(&entry.name).ok()?
        };
        let modified_at = entry.modified.to_system_time().map(DateTime::<Utc>::from);
        Some(RemoteEntry {
            metadata: RemoteMetadata {
                path,
                is_directory: entry.is_directory,
                size_bytes: Some(entry.size),
                etag: None,
                modified_at,
            },
        })
    }
}

#[async_trait]
impl StorageProvider for SmbProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Smb
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
        let mut session = self.session.lock().await;
        let SmbSession { client, tree } = &mut *session;
        client
            .list_directory(tree, "")
            .await
            .map(|_| ())
            .map_err(|error| Self::error(error.to_string()))
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        _cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let mut session = self.session.lock().await;
        let SmbSession { client, tree } = &mut *session;
        let entries = client
            .list_directory(tree, &Self::path(prefix))
            .await
            .map_err(|error| Self::error(error.to_string()))?
            .into_iter()
            .filter(|entry| entry.name != "." && entry.name != "..")
            .filter_map(|entry| Self::entry_metadata(prefix, &entry))
            .collect();
        Ok(Page {
            entries,
            next_cursor: None,
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let mut session = self.session.lock().await;
        let SmbSession { client, tree } = &mut *session;
        let info = client
            .stat(tree, &Self::path(path))
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        Ok(Self::metadata(path.clone(), &info))
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let reader = {
            let session = self.session.lock().await;
            session
                .client
                .open_file_reader(&session.tree, &Self::path(&request.path))
                .await
                .map_err(|error| Self::error(error.to_string()))?
        };
        let start = request.range.as_ref().map_or(0, |range| range.start);
        let end = request.range.as_ref().map(|range| range.end);
        let stream = stream::try_unfold((reader, start, end), |(reader, offset, end)| async move {
            if end.is_some_and(|end| offset >= end) {
                let _ = reader.close().await;
                return Ok(None);
            }
            let length = end
                .map(|end| end.saturating_sub(offset).min(64 * 1024))
                .unwrap_or(64 * 1024);
            if length == 0 {
                let _ = reader.close().await;
                return Ok(None);
            }
            let bytes = reader
                .read_at(offset, length)
                .await
                .map_err(|error| SmbProvider::error(error.to_string()))?;
            if bytes.is_empty() {
                let _ = reader.close().await;
                return Ok(None);
            }
            let next = offset + bytes.len() as u64;
            Ok(Some((Bytes::from(bytes), (reader, next, end))))
        });
        Ok(Box::pin(stream))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let mut writer = {
            let session = self.session.lock().await;
            session
                .client
                .create_file_writer(&session.tree, &Self::path(&request.path))
                .await
                .map_err(|error| Self::error(error.to_string()))?
        };
        let mut content = request.content;
        while let Some(chunk) = content.next().await {
            writer
                .write_chunk(&chunk.map_err(|error| Self::error(error.to_string()))?)
                .await
                .map_err(|error| Self::error(error.to_string()))?;
        }
        writer
            .finish()
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        Ok(RemoteMetadata {
            path: request.path,
            is_directory: false,
            size_bytes: request.size_bytes,
            etag: None,
            modified_at: request.modified_at,
        })
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let mut session = self.session.lock().await;
        let remote_path = Self::path(path);
        let SmbSession { client, tree } = &mut *session;
        let info = client
            .stat(tree, &remote_path)
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        if info.is_directory {
            client.delete_directory(tree, &remote_path).await
        } else {
            client.delete_file(tree, &remote_path).await
        }
        .map_err(|error| Self::error(error.to_string()))
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        let mut session = self.session.lock().await;
        let SmbSession { client, tree } = &mut *session;
        client
            .create_directory(tree, &Self::path(path))
            .await
            .map_err(|error| Self::error(error.to_string()))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let mut session = self.session.lock().await;
        let SmbSession { client, tree } = &mut *session;
        client
            .rename(tree, &Self::path(from), &Self::path(to))
            .await
            .map_err(|error| Self::error(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{SmbConfig, SmbProvider};
    use url::Url;

    #[test]
    fn requires_a_share_in_the_endpoint() {
        let endpoint = Url::parse("smb://server").unwrap();
        assert!(endpoint.path().is_empty());
        let _ = SmbProvider::connect;
        let _ = SmbConfig {
            endpoint,
            username: String::new(),
            password: String::new(),
            domain: String::new(),
        };
    }
}
