#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    #[cfg(target_os = "windows")]
    if std::env::args_os().any(|argument| argument == "--cleanup-windows-integrations") {
        if let Err(error) = bifrost_drive_lib::cleanup_windows_integrations() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = bifrost_drive_lib::run() {
        #[cfg(target_os = "windows")]
        show_startup_error(&error.to_string());
        #[cfg(not(target_os = "windows"))]
        eprintln!("Bifrost Drive could not start: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn show_startup_error(error: &str) {
    use windows::{
        core::PCWSTR,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };

    let message = format!(
        "Bifrost Drive could not start.\n\n{error}\n\nIf this mentions WebView2, install the Microsoft Edge WebView2 Runtime and start Bifrost again."
    );
    let title: Vec<u16> = "Bifrost Drive".encode_utf16().chain(Some(0)).collect();
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_ICONERROR | MB_OK,
        );
    }
}
