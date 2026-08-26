#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
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
