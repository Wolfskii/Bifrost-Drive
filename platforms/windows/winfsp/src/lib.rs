mod engine;
#[cfg(all(target_os = "windows", feature = "native"))]
mod native;

use std::sync::Arc;

use bifrost_storage::StorageProvider;
use thiserror::Error;

pub use engine::{remote_path_from_windows, OpenFile, RemoteFilesystem, WinFspFilesystemError};
#[cfg(all(target_os = "windows", feature = "native"))]
pub use native::{mount, MountHandle};

#[derive(Debug, Error)]
pub enum WinFspError {
    #[error("WinFsp drive mounting is available only on Windows")]
    UnsupportedPlatform,
    #[error("WinFsp 2.1 or later is not installed")]
    RuntimeUnavailable,
    #[error("drive letter must be a single ASCII letter")]
    InvalidDriveLetter,
    #[error("WinFsp initialization failed: {0}")]
    Initialization(String),
    #[error("WinFsp mount failed with NTSTATUS {0:#010x}")]
    Mount(i32),
    #[error("WinFsp callback runtime initialization failed: {0}")]
    Runtime(String),
}

#[derive(Clone)]
pub struct MountConfig {
    pub drive_letter: char,
    pub volume_label: String,
    pub network_drive: bool,
    pub icon_source: Option<String>,
    pub provider: Arc<dyn StorageProvider>,
}

impl MountConfig {
    pub fn normalized_drive_letter(&self) -> Result<char, WinFspError> {
        if !self.drive_letter.is_ascii_alphabetic() {
            return Err(WinFspError::InvalidDriveLetter);
        }
        Ok(self.drive_letter.to_ascii_uppercase())
    }

    pub fn mountpoint(&self) -> Result<String, WinFspError> {
        Ok(format!("{}:", self.normalized_drive_letter()?))
    }
}

#[cfg(all(target_os = "windows", feature = "native"))]
pub fn initialize() -> Result<(), WinFspError> {
    winfsp_wrs::init().map_err(|error| match error {
        winfsp_wrs::InitError::WinFSPNotFound => WinFspError::RuntimeUnavailable,
        other => WinFspError::Initialization(other.to_string()),
    })
}

#[cfg(not(all(target_os = "windows", feature = "native")))]
pub fn initialize() -> Result<(), WinFspError> {
    if cfg!(target_os = "windows") {
        Err(WinFspError::RuntimeUnavailable)
    } else {
        Err(WinFspError::UnsupportedPlatform)
    }
}
