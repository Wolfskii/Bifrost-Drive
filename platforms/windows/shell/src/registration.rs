//! COM and Explorer registration. `DllRegisterServer` writes the per-user keys used during
//! development (`regsvr32`); the installer writes the same keys per machine, and a test
//! keeps the two lists identical.

use windows::{
    core::{Error, Result, PCWSTR},
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY,
            KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
        },
    },
};

pub const CLSID: &str = "{7A3C1F52-5B0E-4D7B-9E34-2C1A6F8B90D1}";
pub const DESCRIPTION: &str = "Bifrost Drive Explorer menu";
pub const HANDLER_NAME: &str = "BifrostDrive";

/// Where Explorer looks for the handler: right-drag onto folders, drives and a folder's
/// background, right-click paste on the same, and right-click Delete on files.
pub const HANDLER_KEYS: [&str; 7] = [
    r"Directory\shellex\DragDropHandlers\BifrostDrive",
    r"Directory\Background\shellex\DragDropHandlers\BifrostDrive",
    r"Drive\shellex\DragDropHandlers\BifrostDrive",
    r"Directory\shellex\ContextMenuHandlers\BifrostDrive",
    r"Directory\Background\shellex\ContextMenuHandlers\BifrostDrive",
    r"Drive\shellex\ContextMenuHandlers\BifrostDrive",
    // Files (folders and drives have their own entries above): for Delete.
    r"*\shellex\ContextMenuHandlers\BifrostDrive",
];

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn check(status: windows::Win32::Foundation::WIN32_ERROR) -> Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::from_hresult(status.to_hresult()))
    }
}

pub fn set_string(root: HKEY, key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let key = wide(key);
    let mut opened = HKEY::default();
    check(unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(key.as_ptr()),
            None,
            windows::core::PWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut opened,
            None,
        )
    })?;
    let name = name.map(wide);
    let data = wide(value);
    let bytes = unsafe {
        std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * size_of::<u16>())
    };
    let status = unsafe {
        RegSetValueExW(
            opened,
            name.as_ref()
                .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr())),
            None,
            REG_SZ,
            Some(bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(opened);
    }
    check(status)
}

pub fn read_string(root: HKEY, key: &str, name: Option<&str>) -> Option<String> {
    let key = wide(key);
    let name = name.map(wide);
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * size_of::<u16>()) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            name.as_ref()
                .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr())),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = (size as usize / size_of::<u16>()).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..length]))
}

pub fn set_dword(root: HKEY, key: &str, name: &str, value: u32) -> Result<()> {
    let key = wide(key);
    let name = wide(name);
    let mut opened = HKEY::default();
    check(unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(key.as_ptr()),
            None,
            windows::core::PWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut opened,
            None,
        )
    })?;
    let status = unsafe {
        RegSetValueExW(
            opened,
            PCWSTR(name.as_ptr()),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    unsafe {
        let _ = RegCloseKey(opened);
    }
    check(status)
}

pub fn delete_tree(root: HKEY, key: &str) -> Result<()> {
    let key = wide(key);
    let status = unsafe { RegDeleteTreeW(root, PCWSTR(key.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status)
}

/// Registers the extension below `classes` (for example `Software\Classes`) of `root`.
pub fn register_under(root: HKEY, classes: &str, dll: &str) -> Result<()> {
    let server = format!(r"{classes}\CLSID\{CLSID}");
    set_string(root, &server, None, DESCRIPTION)?;
    let inproc = format!(r"{server}\InprocServer32");
    set_string(root, &inproc, None, dll)?;
    set_string(root, &inproc, Some("ThreadingModel"), "Apartment")?;
    for key in HANDLER_KEYS {
        set_string(root, &format!(r"{classes}\{key}"), None, CLSID)?;
    }
    Ok(())
}

pub fn unregister_under(root: HKEY, classes: &str) -> Result<()> {
    let mut first_error = None;
    for key in HANDLER_KEYS {
        if let Err(error) = delete_tree(root, &format!(r"{classes}\{key}")) {
            first_error.get_or_insert(error);
        }
    }
    if let Err(error) = delete_tree(root, &format!(r"{classes}\CLSID\{CLSID}")) {
        first_error.get_or_insert(error);
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    /// A private registry area, so tests never touch the real shell registration.
    struct TestRoot(String);

    impl TestRoot {
        /// Tests run in parallel, so each one needs a root of its own.
        fn new(test: &str) -> Self {
            Self(format!(
                r"Software\BifrostDriveShellTest-{}-{test}\Classes",
                std::process::id()
            ))
        }

        fn cleanup(&self) {
            let parent = self.0.trim_end_matches(r"\Classes").to_owned();
            let _ = delete_tree(HKEY_CURRENT_USER, &parent);
        }
    }

    #[test]
    fn registers_every_handler_and_removes_them_again() {
        let root = TestRoot::new("register");

        register_under(HKEY_CURRENT_USER, &root.0, r"C:\Apps\bifrost_shell.dll").unwrap();

        let server = format!(r"{}\CLSID\{CLSID}\InprocServer32", root.0);
        assert_eq!(
            read_string(HKEY_CURRENT_USER, &server, None).as_deref(),
            Some(r"C:\Apps\bifrost_shell.dll")
        );
        assert_eq!(
            read_string(HKEY_CURRENT_USER, &server, Some("ThreadingModel")).as_deref(),
            Some("Apartment")
        );
        for key in HANDLER_KEYS {
            assert_eq!(
                read_string(HKEY_CURRENT_USER, &format!(r"{}\{key}", root.0), None).as_deref(),
                Some(CLSID),
                "{key}"
            );
        }

        unregister_under(HKEY_CURRENT_USER, &root.0).unwrap();

        for key in HANDLER_KEYS {
            assert_eq!(
                read_string(HKEY_CURRENT_USER, &format!(r"{}\{key}", root.0), None),
                None
            );
        }
        assert_eq!(read_string(HKEY_CURRENT_USER, &server, None), None);
        root.cleanup();
    }

    /// The installer writes the machine-wide registration itself (NSIS), so its list has to
    /// match the one `regsvr32` uses.
    #[test]
    fn the_installer_registers_exactly_what_the_dll_registers() {
        let hooks = include_str!("../../../../apps/desktop/src-tauri/windows/winfsp-hooks.nsh");

        assert!(hooks.contains(&format!(r#"!define BIFROST_SHELL_CLSID "{CLSID}""#)));
        for key in HANDLER_KEYS {
            assert_eq!(
                hooks
                    .matches(&format!(r#"BifrostShellHandler "{key}""#))
                    .count(),
                1,
                "register {key}"
            );
            assert_eq!(
                hooks
                    .matches(&format!(r#"BifrostShellHandlerRemove "{key}""#))
                    .count(),
                1,
                "unregister {key}"
            );
        }
        let registered = hooks.matches("!insertmacro BifrostShellHandler \"").count();
        assert_eq!(registered, HANDLER_KEYS.len(), "no extra handlers");
        assert!(hooks.contains("ThreadingModel\" \"Apartment\""));
        assert!(hooks.contains(r"resources\bifrost_windows_shell.dll"));
    }

    #[test]
    fn unregistering_twice_is_not_an_error() {
        let root = TestRoot::new("twice");
        unregister_under(HKEY_CURRENT_USER, &root.0).unwrap();
        unregister_under(HKEY_CURRENT_USER, &root.0).unwrap();
        root.cleanup();
    }
}
