use std::{path::PathBuf, sync::Arc};

use bifrost_common::RemotePath;
use bifrost_storage::StorageError;

use crate::WinFspFilesystemError;

/// The drive operation that failed, as the user would describe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveOperation {
    CreateFile,
    CreateFolder,
    Save,
    Upload,
    Delete,
    Rename,
}

/// Why a drive operation failed, without provider-specific types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveFailureReason {
    PermissionDenied,
    AuthenticationFailed,
    Network(String),
    Unsupported,
    Remote(String),
    Local(String),
}

impl DriveFailureReason {
    /// Classifies an error worth telling the user about; expected outcomes such as a missing
    /// file or a name collision are answered to Windows alone and return `None`.
    pub fn from_error(error: &WinFspFilesystemError) -> Option<Self> {
        match error {
            WinFspFilesystemError::Storage(error) => match error {
                StorageError::PermissionDenied { .. } => Some(Self::PermissionDenied),
                StorageError::AuthenticationFailed { .. } => Some(Self::AuthenticationFailed),
                StorageError::Network { message, .. } => Some(Self::Network(message.clone())),
                StorageError::Io(error) => Some(Self::Network(error.to_string())),
                StorageError::Unsupported { .. } => Some(Self::Unsupported),
                StorageError::Provider { message, .. } => Some(Self::Remote(message.clone())),
                StorageError::NotFound { .. } | StorageError::Cancelled => None,
            },
            WinFspFilesystemError::Staging(message) => Some(Self::Local(message.clone())),
            WinFspFilesystemError::Io(error) => Some(Self::Local(error.to_string())),
            WinFspFilesystemError::InvalidPath(_)
            | WinFspFilesystemError::AlreadyExists(_)
            | WinFspFilesystemError::IsDirectory(_)
            | WinFspFilesystemError::NotDirectory(_)
            | WinFspFilesystemError::DirectoryNotEmpty(_)
            | WinFspFilesystemError::NotWritable(_) => None,
        }
    }

    pub(crate) fn is_transient(&self) -> bool {
        matches!(self, Self::Network(_))
    }
}

/// Where the staged data of an upload that never reached the provider was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadRecovery {
    Saved(PathBuf),
    Lost(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveFailure {
    pub operation: DriveOperation,
    pub path: RemotePath,
    pub reason: DriveFailureReason,
    /// Present only for uploads that failed after Windows had already closed the file.
    pub recovery: Option<UploadRecovery>,
}

/// Receives failures on WinFsp callback threads, so it must return quickly.
pub type DriveFailureSink = Arc<dyn Fn(DriveFailure) + Send + Sync>;

#[derive(Clone)]
pub struct FailureHandling {
    /// Receives staged data whose upload failed; created on first use.
    pub recovery_dir: PathBuf,
    pub sink: Option<DriveFailureSink>,
}

impl Default for FailureHandling {
    fn default() -> Self {
        Self {
            recovery_dir: std::env::temp_dir().join("Bifrost Drive recovered uploads"),
            sink: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_common::ProviderKind;

    #[test]
    fn expected_outcomes_are_not_reported() {
        let path = RemotePath::parse("video.mkv").unwrap();
        assert_eq!(
            DriveFailureReason::from_error(&StorageError::NotFound { path: path.clone() }.into()),
            None
        );
        assert_eq!(
            DriveFailureReason::from_error(&WinFspFilesystemError::AlreadyExists(path.clone())),
            None
        );
        assert_eq!(
            DriveFailureReason::from_error(&StorageError::PermissionDenied { path }.into()),
            Some(DriveFailureReason::PermissionDenied)
        );
    }

    #[test]
    fn only_network_failures_are_retried() {
        let network = StorageError::Network {
            provider: ProviderKind::Sftp,
            message: "connection reset".to_owned(),
        };
        assert!(DriveFailureReason::from_error(&network.into())
            .unwrap()
            .is_transient());
        assert!(!DriveFailureReason::PermissionDenied.is_transient());
    }
}
