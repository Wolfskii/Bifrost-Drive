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
    bifrost_drive_lib::run();
}
