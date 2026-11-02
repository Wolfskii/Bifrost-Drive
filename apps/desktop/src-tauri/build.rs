fn main() {
    #[cfg(target_os = "windows")]
    {
        winfsp_wrs_build::build();
        stage_shell_extension_placeholder();
    }
    tauri_build::build();
}

/// `tauri.windows.conf.json` bundles the Explorer extension as a resource, and `tauri_build`
/// requires every resource to exist when the app is compiled. The real DLL is built, signed and
/// staged over this file by `windows/build-shell.ps1` just before bundling (which fails the
/// bundle if it cannot), so the placeholder only keeps `cargo check` and `cargo test` working.
#[cfg(target_os = "windows")]
fn stage_shell_extension_placeholder() {
    let staged = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("../../../.cache/shell/bifrost_windows_shell.dll");
    if staged.exists() {
        return;
    }
    if let Some(parent) = staged.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&staged, b"");
}
