mod capability;
mod error;
mod path;
mod state;

pub use capability::{Capability, CapabilitySet};
pub use error::{BifrostError, Result};
pub use path::{PathIssue, RemotePath};
pub use state::{ConnectionState, SyncState};

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionId(Uuid);

impl ConnectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(Uuid);

impl TransferId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    S3,
    Sftp,
    WebDav,
    Nextcloud,
    GoogleDrive,
    Ftp,
    Smb,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::S3 => "S3",
            Self::Sftp => "SFTP",
            Self::WebDav => "WebDAV",
            Self::Nextcloud => "Nextcloud",
            Self::GoogleDrive => "Google Drive",
            Self::Ftp => "FTP",
            Self::Smb => "SMB",
        };
        formatter.write_str(name)
    }
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::Sftp => "sftp",
            Self::WebDav => "webdav",
            Self::Nextcloud => "nextcloud",
            Self::GoogleDrive => "google-drive",
            Self::Ftp => "ftp",
            Self::Smb => "smb",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "s3" => Ok(Self::S3),
            "sftp" => Ok(Self::Sftp),
            "webdav" => Ok(Self::WebDav),
            "nextcloud" => Ok(Self::Nextcloud),
            "google-drive" => Ok(Self::GoogleDrive),
            "ftp" => Ok(Self::Ftp),
            "smb" => Ok(Self::Smb),
            _ => Err(BifrostError::Configuration(format!(
                "unknown provider kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMetadata {
    pub path: RemotePath,
    pub is_directory: bool,
    pub size_bytes: Option<u64>,
    pub etag: Option<String>,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
}
