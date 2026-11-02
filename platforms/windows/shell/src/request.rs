//! Handing a request to the running Bifrost Drive app: a small JSON file in the app's own
//! folder, then the app started with `--transfer-request <file>` (a second launch forwards it
//! to the running instance and exits).

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use crate::decision::Request;

/// The app's data folder name and request subfolder. The desktop host
/// (`apps/desktop/src-tauri/src/transfers.rs`) reads the same place.
pub const APP_FOLDER: &str = "com.bifrost.drive";
pub const REQUESTS_FOLDER: &str = "transfer-requests";
pub const REQUEST_ARGUMENT: &str = "--transfer-request";

pub fn requests_dir(local_app_data: &Path) -> PathBuf {
    local_app_data.join(APP_FOLDER).join(REQUESTS_FOLDER)
}

/// Writes the request to a fresh file; an existing file is never reused.
pub fn write_request(directory: &Path, request: &Request) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join(format!("{}.json", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(&json)?;
    file.sync_all()?;
    Ok(path)
}

pub fn launch(app: &Path, request_file: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(app)
        .arg(REQUEST_ARGUMENT)
        .arg(request_file)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::Mode;

    fn request() -> Request {
        Request {
            version: 1,
            mode: Mode::Copy,
            sources: vec![r"C:\a.mkv".to_owned()],
            destination: r"Y:\Filmer".to_owned(),
        }
    }

    #[test]
    fn requests_live_in_the_apps_own_folder() {
        assert_eq!(
            requests_dir(Path::new(r"C:\Users\me\AppData\Local")),
            PathBuf::from(r"C:\Users\me\AppData\Local\com.bifrost.drive\transfer-requests")
        );
    }

    #[test]
    fn every_request_gets_its_own_file() {
        let directory = tempfile::tempdir().unwrap();

        let first = write_request(directory.path(), &request()).unwrap();
        let second = write_request(directory.path(), &request()).unwrap();

        assert_ne!(first, second);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
        assert_eq!(saved["destination"], r"Y:\Filmer");
        assert_eq!(saved["mode"], "copy");
    }
}
