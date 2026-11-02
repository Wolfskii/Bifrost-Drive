//! Tells the transfer engine what kind of drive a Windows path lives on, so it can pick how many
//! files to copy at once: an SSD wants many, a spinning disk wants one ordered stream.
//!
//! The answer is a hint. Whenever Windows cannot say (no permission to query the device, a
//! virtual or pooled disk that reports nothing useful), the class is `Unknown`, which the
//! engine treats with moderate parallelism rather than guessing either way.

use std::path::Path;

use bifrost_transfer::concurrency::StorageClass;

/// The class of the drive `path` is on.
#[cfg(target_os = "windows")]
pub fn classify(path: &Path) -> StorageClass {
    windows_impl::classify(path)
}

/// Other hosts have no adapter yet; the engine falls back to its moderate default.
#[cfg(not(target_os = "windows"))]
pub fn classify(_path: &Path) -> StorageClass {
    StorageClass::Unknown
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::{ffi::c_void, mem::size_of, path::Path};

    use bifrost_transfer::concurrency::StorageClass;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::CloseHandle,
            Storage::FileSystem::{
                CreateFileW, GetDriveTypeW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
                FILE_SHARE_WRITE, OPEN_EXISTING,
            },
            System::{
                Ioctl::{
                    PropertyStandardQuery, StorageDeviceSeekPenaltyProperty,
                    DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY,
                    STORAGE_PROPERTY_QUERY,
                },
                IO::DeviceIoControl,
            },
        },
    };

    // GetDriveTypeW results (winbase.h).
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_RAMDISK: u32 = 6;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub fn classify(path: &Path) -> StorageClass {
        let Some(text) = path.to_str() else {
            return StorageClass::Unknown;
        };
        // \\server\share (but not the \\?\ and \\.\ prefixes that name local devices).
        if text.starts_with(r"\\") && !text.starts_with(r"\\?\") && !text.starts_with(r"\\.\") {
            return StorageClass::Network;
        }
        let text = text
            .strip_prefix(r"\\?\")
            .or_else(|| text.strip_prefix(r"\\.\"))
            .unwrap_or(text);
        let mut characters = text.chars();
        let (Some(letter), Some(':')) = (characters.next(), characters.next()) else {
            return StorageClass::Unknown;
        };
        if !letter.is_ascii_alphabetic() {
            return StorageClass::Unknown;
        }
        let letter = letter.to_ascii_uppercase();
        let root = wide(&format!(r"{letter}:\"));
        match unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } {
            DRIVE_REMOTE => StorageClass::Network,
            DRIVE_REMOVABLE => StorageClass::Removable,
            DRIVE_RAMDISK => StorageClass::Solid,
            DRIVE_FIXED => seek_penalty(letter).unwrap_or(StorageClass::Unknown),
            _ => StorageClass::Unknown,
        }
    }

    /// Asks the storage stack whether the volume's device pays a cost for moving between
    /// positions, which is what separates a spinning disk from flash. Needs no elevation.
    fn seek_penalty(letter: char) -> Option<StorageClass> {
        let device = wide(&format!(r"\\.\{letter}:"));
        let handle = unsafe {
            CreateFileW(
                PCWSTR(device.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .ok()?;
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceSeekPenaltyProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut descriptor = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
        let mut returned = 0u32;
        let asked = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some((&query as *const STORAGE_PROPERTY_QUERY).cast::<c_void>()),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                Some((&mut descriptor as *mut DEVICE_SEEK_PENALTY_DESCRIPTOR).cast::<c_void>()),
                size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
                Some(&mut returned),
                None,
            )
        };
        let _ = unsafe { CloseHandle(handle) };
        asked.ok()?;
        if (returned as usize) < size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() {
            return None;
        }
        Some(if descriptor.IncursSeekPenalty {
            StorageClass::Rotational
        } else {
            StorageClass::Solid
        })
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn network_shares_are_recognised_without_touching_the_network() {
        assert_eq!(
            classify(Path::new(r"\\server\share\folder")),
            StorageClass::Network
        );
    }

    #[test]
    fn a_path_without_a_drive_is_unknown() {
        assert_eq!(
            classify(Path::new(r"relative\folder")),
            StorageClass::Unknown
        );
        assert_eq!(classify(Path::new("")), StorageClass::Unknown);
    }

    #[test]
    fn a_missing_drive_is_unknown() {
        let missing = ('A'..='Z')
            .rev()
            .find(|letter| !Path::new(&format!(r"{letter}:\")).exists())
            .expect("some drive letter is free");

        assert_eq!(
            classify(Path::new(&format!(r"{missing}:\folder"))),
            StorageClass::Unknown
        );
    }

    #[test]
    fn the_system_drive_is_a_local_fixed_disk() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());

        let class = classify(Path::new(&format!(r"{system}\Windows")));

        println!("{system} is classified as {class:?}");
        assert!(
            matches!(
                class,
                StorageClass::Solid | StorageClass::Rotational | StorageClass::Unknown
            ),
            "{class:?}"
        );
    }

    #[test]
    fn long_path_prefixes_are_understood() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());

        assert_eq!(
            classify(Path::new(&format!(r"\\?\{system}\Windows"))),
            classify(Path::new(&format!(r"{system}\Windows")))
        );
    }
}
