fn main() {
    #[cfg(target_os = "windows")]
    winfsp_wrs_build::build();
    tauri_build::build();
}
