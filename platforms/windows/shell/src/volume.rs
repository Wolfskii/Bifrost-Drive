//! Telling a Bifrost drive from any other: its file system is named "Bifrost".

use std::{
    collections::HashMap,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use windows::{core::PCWSTR, Win32::Storage::FileSystem::GetVolumeInformationW};

const FILE_SYSTEM_NAME: &str = "Bifrost";
/// A menu is built per right-click; one answer per drive is enough for a few seconds.
const CACHE_FOR: Duration = Duration::from_secs(5);

static CACHE: Mutex<Option<HashMap<char, (bool, Instant)>>> = Mutex::new(None);

/// Tests have no Bifrost drive, so they can declare one drive letter to be one.
#[cfg(test)]
pub static TEST_DRIVE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Held by every test that sets or depends on `TEST_DRIVE`.
#[cfg(test)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn drive_letter(path: &Path) -> Option<char> {
    let mut characters = path.to_str()?.chars();
    let letter = characters.next()?;
    (letter.is_ascii_alphabetic() && characters.next() == Some(':'))
        .then(|| letter.to_ascii_uppercase())
}

pub fn is_bifrost_drive(path: &Path) -> bool {
    let Some(letter) = drive_letter(path) else {
        return false;
    };
    #[cfg(test)]
    if TEST_DRIVE.load(std::sync::atomic::Ordering::Relaxed) == letter as u32 {
        return true;
    }
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((answer, asked)) = cache.get(&letter) {
        if asked.elapsed() < CACHE_FOR {
            return *answer;
        }
    }
    let answer = filesystem_name(&format!(r"{letter}:\")).as_deref() == Some(FILE_SYSTEM_NAME);
    cache.insert(letter, (answer, Instant::now()));
    answer
}

pub fn filesystem_name(root: &str) -> Option<String> {
    let root: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    let mut name = [0u16; 64];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut name),
        )
        .ok()?;
    }
    let length = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_drive_letter_from_windows_paths() {
        assert_eq!(drive_letter(Path::new(r"y:\Filmer\a.mkv")), Some('Y'));
        assert_eq!(drive_letter(Path::new(r"Y:")), Some('Y'));
        assert_eq!(drive_letter(Path::new(r"\\server\share\a")), None);
        assert_eq!(drive_letter(Path::new(r"relative\a")), None);
    }

    #[test]
    fn the_system_drive_is_not_a_bifrost_drive() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
        assert!(filesystem_name(&format!(r"{system}\")).is_some());
        assert!(!is_bifrost_drive(Path::new(&format!(r"{system}\Windows"))));
    }

    #[test]
    fn a_missing_drive_is_not_a_bifrost_drive() {
        assert_eq!(filesystem_name(r"Q:\Nowhere\"), None);
    }
}
