//! Settings the desktop app publishes for the extension, read each time a menu is built so a
//! change takes effect without restarting Explorer.

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
    },
};

pub const SETTINGS_KEY: &str = r"Software\Bifrost Drive";
pub const MENU_VALUE: &str = "ExplorerTransferMenu";

/// Tests choose the answer so they do not depend on this machine's own setting
/// (-1 reads the registry).
#[cfg(test)]
pub static TEST_OVERRIDE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Whether the menu items are wanted. A missing value means yes: the app only publishes it
/// once it has run, and the items only appear on Bifrost drives anyway.
pub fn menu_enabled() -> bool {
    #[cfg(test)]
    {
        let forced = TEST_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
        if forced >= 0 {
            return forced == 1;
        }
    }
    menu_enabled_at(HKEY_CURRENT_USER, SETTINGS_KEY)
}

pub fn menu_enabled_at(root: HKEY, key: &str) -> bool {
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            w!("ExplorerTransferMenu"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    status != ERROR_SUCCESS || value != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registration::set_dword;

    #[test]
    fn reads_the_published_flag_and_defaults_to_on() {
        let key = format!(
            r"Software\BifrostDriveShellTest-{}-setting",
            std::process::id()
        );
        assert!(menu_enabled_at(HKEY_CURRENT_USER, &key), "missing means on");

        set_dword(HKEY_CURRENT_USER, &key, MENU_VALUE, 0).unwrap();
        assert!(!menu_enabled_at(HKEY_CURRENT_USER, &key));

        set_dword(HKEY_CURRENT_USER, &key, MENU_VALUE, 1).unwrap();
        assert!(menu_enabled_at(HKEY_CURRENT_USER, &key));

        crate::registration::delete_tree(HKEY_CURRENT_USER, &key).unwrap();
    }
}
