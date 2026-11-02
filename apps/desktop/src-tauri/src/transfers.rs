//! Explorer-initiated batch transfers: request intake, drive-letter resolution, the registry
//! of running batches, and the transfer window.
//!
//! The Explorer shell extension writes a small JSON request file and starts the app with
//! `--transfer-request <file>`; the running instance reads it, resolves each Windows path to
//! a local folder or a mounted Bifrost drive, and hands the work to the batch engine.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use bifrost_common::RemotePath;
use bifrost_db::Database;
use bifrost_storage::StorageProvider;
use bifrost_transfer::batch::{
    self, BatchHandle, BatchOptions, BatchRequest, BatchSnapshot, BatchState, Endpoint, Resolution,
    TransferMode,
};
use bifrost_windows_winfsp::MountRefresher;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;

use crate::{DriveMountRegistry, WindowsCredentialStore};

pub const REQUEST_ARGUMENT: &str = "--transfer-request";
pub const WINDOW_LABEL: &str = "transfers";
pub const UPDATE_EVENT: &str = "transfer://update";
/// Folder below the app's local data directory where the shell extension drops requests. The
/// extension (`platforms/windows/shell`) builds the same path on its own.
pub const REQUESTS_FOLDER: &str = "transfer-requests";
const STAGING_FOLDER: &str = "transfer-staging";
const REQUEST_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SOURCES: usize = 100_000;
const FINISHED_KEPT: usize = 20;

#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub version: u32,
    pub mode: TransferMode,
    pub sources: Vec<String>,
    pub destination: String,
}

/// The value after `--transfer-request`, if present.
pub fn request_file_argument(arguments: &[String]) -> Option<PathBuf> {
    let position = arguments
        .iter()
        .position(|argument| argument == REQUEST_ARGUMENT)?;
    arguments.get(position + 1).map(PathBuf::from)
}

pub fn requests_dir(local_data_dir: &Path) -> PathBuf {
    local_data_dir.join(REQUESTS_FOLDER)
}

/// Reads and removes a request file. Only files inside `requests_dir` are accepted, so the
/// command line cannot be used to make the app read arbitrary files.
pub fn read_request(file: &Path, requests_dir: &Path) -> Result<TransferRequest, String> {
    let directory = requests_dir
        .canonicalize()
        .map_err(|_| "The transfer request folder does not exist".to_owned())?;
    let file = file
        .canonicalize()
        .map_err(|_| "The transfer request file does not exist".to_owned())?;
    if file.parent() != Some(directory.as_path()) {
        return Err("The transfer request is not in the expected folder".to_owned());
    }
    let size = std::fs::metadata(&file)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_REQUEST_BYTES {
        let _ = std::fs::remove_file(&file);
        return Err("The transfer request is too large".to_owned());
    }
    let contents = std::fs::read(&file).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(&file);
    let request: TransferRequest = serde_json::from_slice(&contents)
        .map_err(|_| "The transfer request is not valid".to_owned())?;
    validate_request(&request)?;
    Ok(request)
}

fn validate_request(request: &TransferRequest) -> Result<(), String> {
    if request.version != REQUEST_VERSION {
        return Err(
            "This transfer request was made by a different version of Bifrost Drive".to_owned(),
        );
    }
    if request.sources.is_empty() {
        return Err("No files were selected".to_owned());
    }
    if request.sources.len() > MAX_SOURCES {
        return Err("Too many items were selected at once".to_owned());
    }
    for path in request.sources.iter().chain([&request.destination]) {
        let candidate = Path::new(path);
        if !candidate.is_absolute()
            || candidate
                .components()
                .any(|component| component == Component::ParentDir)
        {
            return Err(format!("“{path}” is not a full path"));
        }
    }
    Ok(())
}

/// Where a Windows path lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Local(PathBuf),
    Drive { letter: char, path: RemotePath },
}

/// Classifies `path` as on one of the mounted Bifrost drives or as an ordinary local path.
pub fn locate(path: &str, mounted: &HashSet<char>) -> Result<Location, String> {
    let mut characters = path.chars();
    let letter = characters.next().map(|letter| letter.to_ascii_uppercase());
    match (letter, characters.next()) {
        (Some(letter), Some(':')) if mounted.contains(&letter) => {
            let rest = path[2..].trim_start_matches(['\\', '/']);
            let remote = RemotePath::parse(rest).map_err(|_| {
                format!("“{path}” has a name that this drive cannot store (it contains a colon or a parent folder)")
            })?;
            Ok(Location::Drive {
                letter,
                path: remote,
            })
        }
        _ => Ok(Location::Local(PathBuf::from(path))),
    }
}

fn within(candidate: &Location, parent: &Location) -> bool {
    match (candidate, parent) {
        (Location::Local(candidate), Location::Local(parent)) => {
            let candidate = candidate.to_string_lossy().to_lowercase();
            let parent = parent.to_string_lossy().to_lowercase();
            let parent = parent.trim_end_matches('\\');
            candidate == parent || candidate.starts_with(&format!("{parent}\\"))
        }
        (
            Location::Drive {
                letter: candidate_letter,
                path: candidate,
            },
            Location::Drive {
                letter: parent_letter,
                path: parent,
            },
        ) => {
            let (candidate, parent) = (
                candidate.as_str().to_lowercase(),
                parent.as_str().to_lowercase(),
            );
            candidate_letter == parent_letter
                && (parent.is_empty()
                    || candidate == parent
                    || candidate.starts_with(&format!("{parent}/")))
        }
        _ => false,
    }
}

/// Rejects requests that no drive is part of, and folders copied into themselves.
pub fn validate_locations(sources: &[Location], destination: &Location) -> Result<(), String> {
    for source in sources {
        if matches!(
            (source, destination),
            (Location::Local(_), Location::Local(_))
        ) {
            return Err(
                "Neither the source nor the destination is on a Bifrost Drive, so Windows Explorer should handle this copy"
                    .to_owned(),
            );
        }
        if within(destination, source) {
            return Err("A folder can't be copied or moved into itself".to_owned());
        }
    }
    Ok(())
}

fn direction(sources: &[Location], destination: &Location) -> &'static str {
    match (destination, sources.first()) {
        (Location::Drive { .. }, Some(Location::Local(_))) => "upload",
        (Location::Local(_), Some(Location::Drive { .. })) => "download",
        _ => "transfer",
    }
}

#[derive(Clone, Serialize)]
struct Meta {
    id: u64,
    connection: String,
    direction: &'static str,
    items: usize,
    first_item: String,
    destination: String,
    started_at_ms: u64,
}

#[derive(Serialize)]
pub struct TransferView {
    #[serde(flatten)]
    meta: Meta,
    snapshot: BatchSnapshot,
}

struct Entry {
    handle: BatchHandle,
    meta: Meta,
}

#[derive(Default)]
pub struct TransferManager {
    next_id: AtomicU64,
    batches: Mutex<BTreeMap<u64, Entry>>,
}

impl TransferManager {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Entry>> {
        self.batches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn insert(&self, handle: BatchHandle, mut meta: Meta) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        meta.id = id;
        let mut batches = self.lock();
        // Finished batches the user never dismissed are dropped oldest first.
        let finished = batches
            .iter()
            .filter(|(_, entry)| entry.handle.snapshot().state.is_final())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        if finished.len() >= FINISHED_KEPT {
            for old in &finished[..finished.len() + 1 - FINISHED_KEPT] {
                batches.remove(old);
            }
        }
        batches.insert(id, Entry { handle, meta });
        id
    }

    fn handle(&self, id: u64) -> Result<BatchHandle, String> {
        self.lock()
            .get(&id)
            .map(|entry| entry.handle.clone())
            .ok_or_else(|| "That transfer is no longer listed".to_owned())
    }

    fn views(&self) -> Vec<TransferView> {
        self.lock()
            .values()
            .map(|entry| TransferView {
                meta: entry.meta.clone(),
                snapshot: entry.handle.snapshot(),
            })
            .collect()
    }
}

pub fn list(app: &AppHandle) -> Result<serde_json::Value, String> {
    serde_json::to_value(app.state::<TransferManager>().views()).map_err(|error| error.to_string())
}

pub fn pause(app: &AppHandle, id: u64) -> Result<(), String> {
    app.state::<TransferManager>().handle(id)?.pause();
    Ok(())
}

pub fn resume(app: &AppHandle, id: u64) -> Result<(), String> {
    app.state::<TransferManager>().handle(id)?.resume();
    Ok(())
}

pub fn cancel(app: &AppHandle, id: u64) -> Result<(), String> {
    app.state::<TransferManager>().handle(id)?.cancel();
    Ok(())
}

pub fn answer_conflict(
    app: &AppHandle,
    id: u64,
    resolution: Resolution,
    apply_to_all: bool,
) -> Result<(), String> {
    app.state::<TransferManager>()
        .handle(id)?
        .answer_conflict(resolution, apply_to_all);
    Ok(())
}

/// Removes a finished batch from the list; running ones stay.
pub fn dismiss(app: &AppHandle, id: u64) -> Result<(), String> {
    let manager = app.state::<TransferManager>();
    let mut batches = manager.lock();
    if batches
        .get(&id)
        .is_some_and(|entry| entry.handle.snapshot().state.is_final())
    {
        batches.remove(&id);
    }
    Ok(())
}

pub fn show_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.unminimize();
        window.show().map_err(|error| error.to_string())?;
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App("index.html?window=transfers".into()),
    )
    .title("Bifrost Drive transfers")
    .inner_size(520.0, 400.0)
    .min_inner_size(440.0, 260.0)
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(11, 11, 11, 255))
    .center()
    .focused(true)
    .build()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

struct Prepared {
    request: BatchRequest,
    meta: Meta,
    /// Folders to refresh on each drive once the batch ends.
    refreshes: Vec<(MountRefresher, Vec<RemotePath>)>,
}

async fn prepare(app: &AppHandle, request: &TransferRequest) -> Result<Prepared, String> {
    let database = app.state::<Database>();
    let registry = app.state::<DriveMountRegistry>();
    let credentials = app.state::<WindowsCredentialStore>();
    let mut mounted = HashMap::new();
    for connection in database
        .list_connections()
        .await
        .map_err(|error| error.to_string())?
    {
        if let Ok(Some(letter)) = crate::configured_drive_letter(&connection.configuration_json) {
            if registry.contains(&connection.id.to_string()) {
                mounted.insert(letter, connection);
            }
        }
    }
    let letters = mounted.keys().copied().collect::<HashSet<_>>();
    let destination = locate(&request.destination, &letters)?;
    let sources = request
        .sources
        .iter()
        .map(|source| locate(source, &letters))
        .collect::<Result<Vec<_>, _>>()?;
    validate_locations(&sources, &destination)?;

    let mut providers: HashMap<char, Arc<dyn StorageProvider>> = HashMap::new();
    for location in sources.iter().chain([&destination]) {
        let Location::Drive { letter, .. } = location else {
            continue;
        };
        if providers.contains_key(letter) {
            continue;
        }
        let provider =
            crate::provider_for_connection(&mounted[letter], credentials.inner()).await?;
        providers.insert(*letter, Arc::from(provider));
    }
    let endpoint = |location: &Location| match location {
        Location::Local(path) => Endpoint::Local(path.clone()),
        Location::Drive { letter, path } => Endpoint::Remote {
            provider: Arc::clone(&providers[letter]),
            path: path.clone(),
        },
    };

    let mut changed: BTreeMap<char, Vec<RemotePath>> = BTreeMap::new();
    if let Location::Drive { letter, path } = &destination {
        changed.entry(*letter).or_default().push(path.clone());
    }
    if request.mode == TransferMode::Move {
        for source in &sources {
            if let Location::Drive { letter, path } = source {
                changed.entry(*letter).or_default().push(path.clone());
            }
        }
    }
    let refreshes = changed
        .into_iter()
        .filter_map(|(letter, paths)| {
            registry
                .refresher(&mounted[&letter].id.to_string())
                .map(|refresher| (refresher, paths))
        })
        .collect();

    let connection = match (&destination, sources.first()) {
        (Location::Drive { letter, .. }, _) | (_, Some(Location::Drive { letter, .. })) => {
            mounted[letter].name.clone()
        }
        _ => String::new(),
    };
    let first_item = Path::new(&request.sources[0])
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.sources[0].clone());
    Ok(Prepared {
        request: BatchRequest {
            sources: sources.iter().map(endpoint).collect(),
            destination: endpoint(&destination),
            mode: request.mode,
        },
        meta: Meta {
            id: 0,
            connection,
            direction: direction(&sources, &destination),
            items: request.sources.len(),
            first_item,
            destination: request.destination.clone(),
            started_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
        },
        refreshes,
    })
}

/// Reads a request the shell extension left behind and runs it, reporting problems the user
/// can act on with a notification because nothing else is on screen yet.
pub async fn run_request_file(app: AppHandle, file: PathBuf) {
    let local_data_dir = match app.path().app_local_data_dir() {
        Ok(directory) => directory,
        Err(error) => return notify_problem(&app, &error.to_string()),
    };
    let request = match read_request(&file, &requests_dir(&local_data_dir)) {
        Ok(request) => request,
        Err(message) => return notify_problem(&app, &message),
    };
    let prepared = match prepare(&app, &request).await {
        Ok(prepared) => prepared,
        Err(message) => return notify_problem(&app, &message),
    };
    let staging = local_data_dir.join(STAGING_FOLDER);
    if let Err(error) = tokio::fs::create_dir_all(&staging).await {
        return notify_problem(&app, &error.to_string());
    }
    let handle = batch::start(
        prepared.request,
        BatchOptions {
            temp_dir: staging,
            ..BatchOptions::default()
        },
    );
    let meta = prepared.meta;
    let id = app
        .state::<TransferManager>()
        .insert(handle.clone(), meta.clone());
    if let Err(message) = show_window(&app) {
        tracing::warn!(%message, "could not open the transfer window");
    }
    watch(
        app,
        handle,
        Meta { id, ..meta },
        prepared.refreshes,
        request.mode,
    )
    .await;
}

fn notify_problem(app: &AppHandle, message: &str) {
    tracing::warn!(%message, "could not start an Explorer transfer");
    let _ = app
        .notification()
        .builder()
        .title("Couldn't start the transfer")
        .body(message)
        .show();
}

async fn watch(
    app: AppHandle,
    handle: BatchHandle,
    meta: Meta,
    refreshes: Vec<(MountRefresher, Vec<RemotePath>)>,
    mode: TransferMode,
) {
    let mut updates = handle.subscribe();
    let mut last = None;
    loop {
        let snapshot = updates.borrow_and_update().clone();
        if last.as_ref() != Some(&snapshot) {
            let view = TransferView {
                meta: meta.clone(),
                snapshot: snapshot.clone(),
            };
            let _ = app.emit_to(WINDOW_LABEL, UPDATE_EVENT, &view);
            last = Some(snapshot.clone());
        }
        if snapshot.state.is_final() || updates.changed().await.is_err() {
            break;
        }
    }
    let snapshot = handle.snapshot();
    for (refresher, paths) in refreshes {
        refresher.refresh(&paths).await;
    }
    finish(&app, &meta, &snapshot, mode).await;
}

fn plural(count: u64, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// The outcome in plain words for notifications and the activity history.
pub fn describe_result(
    snapshot: &BatchSnapshot,
    mode: TransferMode,
    destination: &str,
) -> (String, String) {
    let verb = match mode {
        TransferMode::Copy => "copied",
        TransferMode::Move => "moved",
    };
    let headline = match snapshot.state {
        BatchState::Completed => format!(
            "{} {verb} to {destination}",
            plural(snapshot.done_files, "file")
        ),
        BatchState::CompletedWithErrors => format!(
            "{} of {} {verb}; {} didn't transfer",
            snapshot.done_files,
            plural(snapshot.total_files, "file"),
            snapshot.failed_files
        ),
        BatchState::Cancelled => format!(
            "Transfer to {destination} was cancelled after {}",
            plural(snapshot.done_files, "file")
        ),
        _ => format!("The transfer to {destination} couldn't start"),
    };
    let detail = if snapshot.state == BatchState::CompletedWithErrors && mode == TransferMode::Move
    {
        "Files that didn't transfer were left where they were. Open the transfer window for details."
    } else if snapshot.state == BatchState::CompletedWithErrors {
        "Open the transfer window for details."
    } else {
        ""
    };
    (headline, detail.to_owned())
}

async fn finish(app: &AppHandle, meta: &Meta, snapshot: &BatchSnapshot, mode: TransferMode) {
    let (headline, detail) = describe_result(snapshot, mode, &meta.destination);
    let (kind, status) = match snapshot.state {
        BatchState::Completed => ("transfer_completed", "completed"),
        BatchState::Cancelled => ("transfer_cancelled", "cancelled"),
        _ => ("transfer_failed", "failed"),
    };
    let details = if detail.is_empty() {
        headline.clone()
    } else {
        format!("{headline}. {detail}")
    };
    if let Err(error) = app
        .state::<Database>()
        .insert_activity(kind, Some(&details), status)
        .await
    {
        tracing::warn!(%error, "could not record the transfer in activity history");
    }
    let window_visible = app
        .get_webview_window(WINDOW_LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let needs_attention = !matches!(
        snapshot.state,
        BatchState::Completed | BatchState::Cancelled
    );
    if needs_attention || !window_visible {
        let _ = app
            .notification()
            .builder()
            .title(if needs_attention {
                "Some files weren't transferred"
            } else {
                "Transfer finished"
            })
            .body(if detail.is_empty() {
                headline
            } else {
                format!("{headline}. {detail}")
            })
            .show();
    }
}

/// The registry flag the shell extension reads each time it builds a menu, so switching the
/// setting takes effect without restarting Explorer.
pub fn set_menu_enabled(enabled: bool) -> Result<(), String> {
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
                KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
            },
        },
    };
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Bifrost Drive"),
            None,
            windows::core::PWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("could not open the settings key: {}", status.0));
    }
    let value = u32::from(enabled).to_le_bytes();
    let status = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(w!("ExplorerTransferMenu").as_ptr()),
            None,
            REG_DWORD,
            Some(&value),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if status != ERROR_SUCCESS {
        return Err(format!("could not save the setting: {}", status.0));
    }
    Ok(())
}

/// Removes the settings key on uninstall.
pub fn remove_menu_setting() -> Result<(), String> {
    use windows::{
        core::w,
        Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
            System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER},
        },
    };
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, w!(r"Software\Bifrost Drive")) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("could not remove the settings key: {}", status.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letters(letters: &[char]) -> HashSet<char> {
        letters.iter().copied().collect()
    }

    fn request_json(mode: &str, sources: &[&str], destination: &str) -> String {
        serde_json::json!({
            "version": 1,
            "mode": mode,
            "sources": sources,
            "destination": destination,
        })
        .to_string()
    }

    #[test]
    fn finds_the_request_file_after_the_argument() {
        let arguments = ["bifrost-drive.exe", "--transfer-request", r"C:\x\r.json"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            request_file_argument(&arguments),
            Some(PathBuf::from(r"C:\x\r.json"))
        );
        assert_eq!(request_file_argument(&arguments[..2]), None);
    }

    #[test]
    fn reads_a_request_once_and_removes_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("r.json");
        std::fs::write(
            &file,
            request_json("move", &[r"C:\Users\me\clip.mkv"], r"Y:\Filmer"),
        )
        .unwrap();

        let request = read_request(&file, directory.path()).unwrap();

        assert_eq!(request.mode, TransferMode::Move);
        assert_eq!(request.sources, vec![r"C:\Users\me\clip.mkv".to_owned()]);
        assert!(!file.exists());
    }

    #[test]
    fn refuses_request_files_outside_the_requests_folder() {
        let directory = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let file = elsewhere.path().join("r.json");
        std::fs::write(&file, request_json("copy", &[r"C:\a"], r"Y:\b")).unwrap();

        assert!(read_request(&file, directory.path()).is_err());
        assert!(file.exists(), "a refused file must not be deleted");
    }

    #[test]
    fn refuses_malformed_or_unsafe_requests() {
        let directory = tempfile::tempdir().unwrap();
        let check = |body: String| {
            let file = directory.path().join("r.json");
            std::fs::write(&file, body).unwrap();
            read_request(&file, directory.path())
        };
        assert!(check("not json".to_owned()).is_err());
        assert!(check(request_json("copy", &[], r"Y:\b")).is_err());
        assert!(check(request_json("copy", &["relative.txt"], r"Y:\b")).is_err());
        assert!(check(request_json("copy", &[r"C:\a\..\b"], r"Y:\b")).is_err());
        assert!(check(request_json("copy", &[r"C:\a"], r"Y:\b")).is_ok());
        assert!(check(
            request_json("copy", &[r"C:\a"], r"Y:\b").replace("\"version\":1", "\"version\":9")
        )
        .is_err());
    }

    #[test]
    fn tells_drive_paths_from_local_paths() {
        let mounted = letters(&['Y']);

        assert_eq!(
            locate(r"y:\Filmer\Captures", &mounted).unwrap(),
            Location::Drive {
                letter: 'Y',
                path: RemotePath::parse("Filmer/Captures").unwrap()
            }
        );
        assert_eq!(
            locate(r"Y:\", &mounted).unwrap(),
            Location::Drive {
                letter: 'Y',
                path: RemotePath::root()
            }
        );
        assert_eq!(
            locate(r"C:\Users\me", &mounted).unwrap(),
            Location::Local(PathBuf::from(r"C:\Users\me"))
        );
        assert!(locate(r"Y:\clip: 1.mkv", &mounted).is_err());
    }

    #[test]
    fn needs_a_drive_on_one_side_and_refuses_copying_into_itself() {
        let mounted = letters(&['Y']);
        let local = |path| locate(path, &mounted).unwrap();

        assert!(validate_locations(&[local(r"C:\a")], &local(r"D:\b")).is_err());
        assert!(validate_locations(&[local(r"C:\a")], &local(r"Y:\b")).is_ok());
        assert!(validate_locations(&[local(r"Y:\a")], &local(r"C:\b")).is_ok());
        assert!(validate_locations(&[local(r"Y:\a")], &local(r"Y:\a\sub")).is_err());
        assert!(validate_locations(&[local(r"Y:\a")], &local(r"Y:\ab")).is_ok());
        assert!(validate_locations(&[local(r"Y:\a")], &local(r"Y:\A")).is_err());
    }

    #[test]
    fn describes_results_in_plain_words() {
        let mut snapshot = BatchSnapshot {
            state: BatchState::Completed,
            mode: TransferMode::Move,
            total_files: 2,
            total_bytes: 10,
            done_files: 2,
            done_bytes: 10,
            skipped_files: 0,
            failed_files: 0,
            failures: Vec::new(),
            current: Vec::new(),
            bytes_per_second: 0,
            seconds_left: None,
            conflict: None,
        };
        assert_eq!(
            describe_result(&snapshot, TransferMode::Move, r"Y:\Filmer").0,
            r"2 files moved to Y:\Filmer"
        );

        snapshot.state = BatchState::CompletedWithErrors;
        snapshot.done_files = 1;
        snapshot.failed_files = 1;
        let (headline, detail) = describe_result(&snapshot, TransferMode::Move, r"Y:\Filmer");
        assert_eq!(headline, "1 of 2 files moved; 1 didn't transfer");
        assert!(detail.contains("left where they were"));
    }
}
