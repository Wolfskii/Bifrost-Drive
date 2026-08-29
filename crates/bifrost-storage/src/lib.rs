use async_trait::async_trait;
use bifrost_common::{CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::{ops::Range, pin::Pin};
use thiserror::Error;
use url::Url;

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;
pub type WriteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub endpoint: Url,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub metadata: RemoteMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub entries: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub path: RemotePath,
    pub range: Option<Range<u64>>,
}

pub struct WriteRequest {
    pub path: RemotePath,
    pub content: WriteStream,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockToken {
    pub token: String,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("provider {provider} rejected authentication")]
    AuthenticationFailed { provider: ProviderKind },
    #[error("provider {provider} rejected the request: {message}")]
    Provider {
        provider: ProviderKind,
        message: String,
    },
    #[error("network error for {provider}: {message}")]
    Network {
        provider: ProviderKind,
        message: String,
    },
    #[error("remote item was not found: {path}")]
    NotFound { path: RemotePath },
    #[error("permission denied for remote item: {path}")]
    PermissionDenied { path: RemotePath },
    #[error("capability {capability} is not supported by {provider}")]
    Unsupported {
        provider: ProviderKind,
        capability: String,
    },
    #[error("transfer was cancelled")]
    Cancelled,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageCapacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[async_trait]
pub trait StorageProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> CapabilitySet;

    fn capabilities_for_path(&self, _path: &RemotePath) -> CapabilitySet {
        self.capabilities()
    }

    async fn test_connection(&self) -> Result<(), StorageError>;
    async fn list(
        &self,
        prefix: &RemotePath,
        cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError>;
    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError>;
    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError>;
    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError>;
    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError>;

    async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
        Ok(None)
    }

    async fn create_directory(&self, _path: &RemotePath) -> Result<(), StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "create_directory".to_owned(),
        })
    }

    async fn rename(&self, _from: &RemotePath, _to: &RemotePath) -> Result<(), StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "rename".to_owned(),
        })
    }

    async fn replace(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        self.delete(to).await?;
        self.rename(from, to).await
    }

    async fn copy(&self, _from: &RemotePath, _to: &RemotePath) -> Result<(), StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "server_side_copy".to_owned(),
        })
    }

    async fn lock(
        &self,
        _path: &RemotePath,
        _owner: &str,
        _timeout_seconds: u64,
    ) -> Result<LockToken, StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "locking".to_owned(),
        })
    }

    async fn unlock(&self, _path: &RemotePath, _token: &LockToken) -> Result<(), StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "locking".to_owned(),
        })
    }
}
