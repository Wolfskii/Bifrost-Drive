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
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bifrost_common::RemotePath;
use bifrost_db::{Database, NewTransferHistory, TransferHistoryUpdate};
use bifrost_storage::StorageProvider;
use bifrost_transfer::{
    batch::{
        self, BatchHandle, BatchOptions, BatchRequest, BatchSnapshot, BatchState, DeleteRequest,
        Endpoint, Resolution, TransferMode,
    },
    concurrency::{tune, Involves, StorageClass},
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
/// How long a clean "100%" stays on screen before the window closes itself.
const AUTO_CLOSE_DELAY: Duration = Duration::from_millis(1500);
/// How many of the selected paths the log keeps for one batch.
const HISTORY_ITEMS_KEPT: usize = 200;
const HISTORY_LIMIT: i64 = 500;

#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub version: u32,
    pub mode: TransferMode,
    pub sources: Vec<String>,
    /// The receiving folder; a delete has none.
    #[serde(default)]
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
    let destination = (request.mode != TransferMode::Delete).then_some(&request.destination);
    for path in request.sources.iter().chain(destination) {
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

/// Rejects folders copied into themselves. Transfers between ordinary folders are accepted
/// too: the Explorer menu offers Paste everywhere, and the engine copies local files itself.
pub fn validate_locations(sources: &[Location], destination: &Location) -> Result<(), String> {
    for source in sources {
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
    /// The row in the transfer log, or 0 when it could not be written.
    history_id: i64,
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

    /// Running and finished batches by their log row, so the log can show live progress.
    fn live(&self) -> HashMap<i64, (u64, BatchSnapshot)> {
        self.lock()
            .values()
            .filter(|entry| entry.meta.history_id > 0)
            .map(|entry| {
                (
                    entry.meta.history_id,
                    (entry.meta.id, entry.handle.snapshot()),
                )
            })
            .collect()
    }

    /// True when nothing is running and nothing needs the user's attention.
    fn all_clean(&self) -> bool {
        self.lock().values().all(|entry| {
            matches!(
                entry.handle.snapshot().state,
                BatchState::Completed | BatchState::Cancelled
            )
        })
    }

    fn remove_finished(&self) {
        self.lock()
            .retain(|_, entry| !entry.handle.snapshot().state.is_final());
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
    .inner_size(700.0, 290.0)
    .min_inner_size(560.0, 250.0)
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(11, 11, 11, 255))
    .center()
    .focused(true)
    .build()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

enum Work {
    Batch(BatchRequest),
    Delete(DeleteRequest),
}

struct Prepared {
    work: Work,
    /// Parallelism and ordering chosen for the drives this request touches.
    options: BatchOptions,
    meta: Meta,
    /// Folders to refresh on each drive once the batch ends.
    refreshes: Vec<(MountRefresher, Vec<RemotePath>)>,
}

/// What the request reads from and writes to, for choosing how many files to work on at once.
/// Explorer selections come from one folder, so the first source stands for all of them.
async fn involves(sources: &[Location], destination: Option<&Location>) -> Involves {
    let mut local = Vec::new();
    for location in sources.first().into_iter().chain(destination) {
        if let Location::Local(path) = location {
            let path = path.clone();
            local.push(
                tokio::task::spawn_blocking(move || bifrost_windows_storage::classify(&path))
                    .await
                    .unwrap_or(StorageClass::Unknown),
            );
        }
    }
    Involves {
        local,
        remote: sources
            .iter()
            .chain(destination)
            .any(|location| matches!(location, Location::Drive { .. })),
    }
}

async fn prepare(
    app: &AppHandle,
    request: &TransferRequest,
    staging: &Path,
) -> Result<Prepared, String> {
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
    let deleting = request.mode == TransferMode::Delete;
    let sources = request
        .sources
        .iter()
        .map(|source| locate(source, &letters))
        .collect::<Result<Vec<_>, _>>()?;
    let destination = if deleting {
        if sources
            .iter()
            .any(|source| matches!(source, Location::Local(_)))
        {
            return Err(
                "Only items on a Bifrost drive can be deleted this way; Windows deletes everything else"
                    .to_owned(),
            );
        }
        None
    } else {
        let destination = locate(&request.destination, &letters)?;
        validate_locations(&sources, &destination)?;
        Some(destination)
    };

    let mut providers: HashMap<char, Arc<dyn StorageProvider>> = HashMap::new();
    for location in sources.iter().chain(destination.as_ref()) {
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
    if let Some(Location::Drive { letter, path }) = &destination {
        changed.entry(*letter).or_default().push(path.clone());
    }
    if matches!(request.mode, TransferMode::Move | TransferMode::Delete) {
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
        (Some(Location::Drive { letter, .. }), _) | (_, Some(Location::Drive { letter, .. })) => {
            mounted[letter].name.clone()
        }
        _ => String::new(),
    };
    let first_item = Path::new(&request.sources[0])
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.sources[0].clone());
    let involves = involves(&sources, destination.as_ref()).await;
    let options = tune(
        BatchOptions {
            temp_dir: staging.to_path_buf(),
            ..BatchOptions::default()
        },
        &involves,
    );
    let (work, direction, shown_destination) = match destination {
        Some(destination) => (
            Work::Batch(BatchRequest {
                sources: sources.iter().map(endpoint).collect(),
                destination: endpoint(&destination),
                mode: request.mode,
            }),
            direction(&sources, &destination),
            request.destination.clone(),
        ),
        None => (
            Work::Delete(DeleteRequest {
                targets: sources.iter().map(endpoint).collect(),
            }),
            "delete",
            // Where the deleted items were, for the result messages.
            Path::new(&request.sources[0]).parent().map_or_else(
                || request.sources[0].clone(),
                |parent| parent.to_string_lossy().into_owned(),
            ),
        ),
    };
    Ok(Prepared {
        work,
        options,
        meta: Meta {
            id: 0,
            history_id: 0,
            connection,
            direction,
            items: request.sources.len(),
            first_item,
            destination: shown_destination,
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
    let staging = local_data_dir.join(STAGING_FOLDER);
    if let Err(error) = tokio::fs::create_dir_all(&staging).await {
        return notify_problem(&app, &error.to_string());
    }
    let prepared = match prepare(&app, &request, &staging).await {
        Ok(prepared) => prepared,
        Err(message) => {
            record_failed_start(&app, &request, &message).await;
            return notify_problem(&app, &message);
        }
    };
    let handle = match prepared.work {
        Work::Batch(batch_request) => batch::start(batch_request, prepared.options),
        Work::Delete(delete_request) => batch::start_delete(delete_request, prepared.options),
    };
    let mut meta = prepared.meta;
    meta.history_id = record_start(&app, &request, &meta).await;
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
    if mode == TransferMode::Delete {
        return describe_delete(snapshot, destination);
    }
    let verb = match mode {
        TransferMode::Copy => "copied",
        TransferMode::Move => "moved",
        TransferMode::Delete => unreachable!("handled above"),
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

fn describe_delete(snapshot: &BatchSnapshot, folder: &str) -> (String, String) {
    let headline = match snapshot.state {
        BatchState::Completed => format!(
            "{} deleted from {folder}",
            plural(snapshot.done_files, "file")
        ),
        BatchState::CompletedWithErrors => format!(
            "{} of {} deleted from {folder}; {} couldn't be deleted",
            snapshot.done_files,
            plural(snapshot.total_files, "file"),
            snapshot.failed_files
        ),
        BatchState::Cancelled => format!(
            "Deleting from {folder} was cancelled after {}",
            plural(snapshot.done_files, "file")
        ),
        _ => format!("Deleting from {folder} couldn't start"),
    };
    let detail = if snapshot.state == BatchState::CompletedWithErrors {
        "Items that couldn't be deleted are still there. Open the transfer window for details."
    } else {
        ""
    };
    (headline, detail.to_owned())
}

fn mode_name(mode: TransferMode) -> &'static str {
    match mode {
        TransferMode::Copy => "copy",
        TransferMode::Move => "move",
        TransferMode::Delete => "delete",
    }
}

fn state_name(state: BatchState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "failed".to_owned())
}

/// Adds the batch to the transfer log and returns its row, or 0 if the log is unavailable.
async fn record_start(app: &AppHandle, request: &TransferRequest, meta: &Meta) -> i64 {
    let entry = NewTransferHistory {
        mode: mode_name(request.mode).to_owned(),
        direction: meta.direction.to_owned(),
        connection: meta.connection.clone(),
        destination: meta.destination.clone(),
        items: request
            .sources
            .iter()
            .take(HISTORY_ITEMS_KEPT)
            .cloned()
            .collect(),
        item_count: request.sources.len() as i64,
    };
    match app
        .state::<Database>()
        .insert_transfer_history(&entry)
        .await
    {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(%error, "could not record the transfer in the log");
            0
        }
    }
}

/// A request that could not even start is still worth a line in the log.
async fn record_failed_start(app: &AppHandle, request: &TransferRequest, message: &str) {
    let database = app.state::<Database>();
    let entry = NewTransferHistory {
        mode: mode_name(request.mode).to_owned(),
        direction: "transfer".to_owned(),
        connection: String::new(),
        destination: request.destination.clone(),
        items: request
            .sources
            .iter()
            .take(HISTORY_ITEMS_KEPT)
            .cloned()
            .collect(),
        item_count: request.sources.len() as i64,
    };
    let Ok(id) = database.insert_transfer_history(&entry).await else {
        return;
    };
    let failures =
        serde_json::json!([{ "name": "", "reason": { "kind": "other", "detail": message } }]);
    let _ = database
        .finish_transfer_history(
            id,
            &TransferHistoryUpdate {
                state: "failed".to_owned(),
                total_files: 0,
                done_files: 0,
                skipped_files: 0,
                failed_files: 0,
                total_bytes: 0,
                done_bytes: 0,
                failures_json: failures.to_string(),
            },
        )
        .await;
}

async fn record_finish(app: &AppHandle, meta: &Meta, snapshot: &BatchSnapshot) {
    if meta.history_id <= 0 {
        return;
    }
    let update = TransferHistoryUpdate {
        state: state_name(snapshot.state),
        total_files: snapshot.total_files as i64,
        done_files: snapshot.done_files as i64,
        skipped_files: snapshot.skipped_files as i64,
        failed_files: snapshot.failed_files as i64,
        total_bytes: snapshot.total_bytes as i64,
        done_bytes: snapshot.done_bytes as i64,
        failures_json: serde_json::to_string(&snapshot.failures)
            .unwrap_or_else(|_| "[]".to_owned()),
    };
    if let Err(error) = app
        .state::<Database>()
        .finish_transfer_history(meta.history_id, &update)
        .await
    {
        tracing::warn!(%error, "could not update the transfer log");
    }
}

/// Closes the transfer window a moment after everything finished cleanly. The finished cards
/// are cleared too (the log keeps them), so the next transfer opens a fresh window instead of
/// one stacked with old results.
fn close_when_clean(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(AUTO_CLOSE_DELAY).await;
        let manager = app.state::<TransferManager>();
        // Something else may have started, or failed, while the 100% was showing.
        if !manager.all_clean() {
            return;
        }
        manager.remove_finished();
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.close();
        }
    });
}

async fn finish(app: &AppHandle, meta: &Meta, snapshot: &BatchSnapshot, mode: TransferMode) {
    record_finish(app, meta, snapshot).await;
    let (headline, detail) = describe_result(snapshot, mode, &meta.destination);
    let deleting = mode == TransferMode::Delete;
    let (kind, status) = match (snapshot.state, deleting) {
        (BatchState::Completed, false) => ("transfer_completed", "completed"),
        (BatchState::Cancelled, false) => ("transfer_cancelled", "cancelled"),
        (_, false) => ("transfer_failed", "failed"),
        (BatchState::Completed, true) => ("delete_completed", "completed"),
        (BatchState::Cancelled, true) => ("delete_cancelled", "cancelled"),
        (_, true) => ("delete_failed", "failed"),
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
    if snapshot.state == BatchState::Completed {
        close_when_clean(app.clone());
    }
    if needs_attention || !window_visible {
        let _ = app
            .notification()
            .builder()
            .title(match (needs_attention, deleting) {
                (true, false) => "Some files weren't transferred",
                (false, false) => "Transfer finished",
                (true, true) => "Some items weren't deleted",
                (false, true) => "Delete finished",
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

/// Progress of a batch that is still running, shown on its row in the log.
#[derive(Serialize)]
struct LiveProgress {
    /// The id the transfer window and its controls use.
    transfer_id: u64,
    bytes_per_second: u64,
    files_per_second: u64,
    seconds_left: Option<u64>,
    current: Vec<String>,
}

#[derive(Serialize)]
struct HistoryEntry {
    id: i64,
    started_at: String,
    finished_at: Option<String>,
    mode: String,
    direction: String,
    connection: String,
    destination: String,
    items: Vec<String>,
    item_count: i64,
    state: String,
    total_files: i64,
    done_files: i64,
    skipped_files: i64,
    failed_files: i64,
    total_bytes: i64,
    done_bytes: i64,
    failures: serde_json::Value,
    live: Option<LiveProgress>,
}

/// The transfer log, newest first, with running batches showing their current progress.
pub async fn history(app: &AppHandle) -> Result<serde_json::Value, String> {
    let rows = app
        .state::<Database>()
        .list_transfer_history(HISTORY_LIMIT)
        .await
        .map_err(|error| error.to_string())?;
    let live = app.state::<TransferManager>().live();
    let entries = rows
        .into_iter()
        .map(|row| {
            let running = live
                .get(&row.id)
                .filter(|(_, snapshot)| !snapshot.state.is_final());
            let mut entry = HistoryEntry {
                failures: serde_json::from_str(&row.failures_json)
                    .unwrap_or_else(|_| serde_json::json!([])),
                id: row.id,
                started_at: row.started_at,
                finished_at: row.finished_at,
                mode: row.mode,
                direction: row.direction,
                connection: row.connection,
                destination: row.destination,
                items: row.items,
                item_count: row.item_count,
                state: row.state,
                total_files: row.total_files,
                done_files: row.done_files,
                skipped_files: row.skipped_files,
                failed_files: row.failed_files,
                total_bytes: row.total_bytes,
                done_bytes: row.done_bytes,
                live: None,
            };
            if let Some((transfer_id, snapshot)) = running {
                entry.state = state_name(snapshot.state);
                entry.total_files = snapshot.total_files as i64;
                entry.done_files = snapshot.done_files as i64;
                entry.skipped_files = snapshot.skipped_files as i64;
                entry.failed_files = snapshot.failed_files as i64;
                entry.total_bytes = snapshot.total_bytes as i64;
                entry.done_bytes = snapshot.done_bytes as i64;
                entry.failures = serde_json::to_value(&snapshot.failures)
                    .unwrap_or_else(|_| serde_json::json!([]));
                entry.live = Some(LiveProgress {
                    transfer_id: *transfer_id,
                    bytes_per_second: snapshot.bytes_per_second,
                    files_per_second: snapshot.files_per_second,
                    seconds_left: snapshot.seconds_left,
                    current: snapshot.current.clone(),
                });
            }
            entry
        })
        .collect::<Vec<_>>();
    serde_json::to_value(entries).map_err(|error| error.to_string())
}

/// Clears the finished entries from the log; running batches stay.
pub async fn clear_history(app: &AppHandle) -> Result<u64, String> {
    app.state::<Database>()
        .clear_finished_transfer_history()
        .await
        .map_err(|error| error.to_string())
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
    fn accepts_any_pair_of_locations_but_refuses_copying_into_itself() {
        let mounted = letters(&['Y']);
        let local = |path| locate(path, &mounted).unwrap();

        assert!(validate_locations(&[local(r"C:\a")], &local(r"D:\b")).is_ok());
        assert!(validate_locations(&[local(r"C:\a")], &local(r"C:\a\sub")).is_err());
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
            files_per_second: 0,
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

    #[test]
    fn describes_deletions_in_plain_words() {
        let mut snapshot = BatchSnapshot {
            state: BatchState::Completed,
            mode: TransferMode::Delete,
            total_files: 3,
            total_bytes: 0,
            done_files: 3,
            done_bytes: 0,
            skipped_files: 0,
            failed_files: 0,
            failures: Vec::new(),
            current: Vec::new(),
            bytes_per_second: 0,
            files_per_second: 0,
            seconds_left: None,
            conflict: None,
        };
        assert_eq!(
            describe_result(&snapshot, TransferMode::Delete, r"Y:\Filmer").0,
            r"3 files deleted from Y:\Filmer"
        );

        snapshot.state = BatchState::CompletedWithErrors;
        snapshot.done_files = 2;
        snapshot.failed_files = 1;
        let (headline, detail) = describe_result(&snapshot, TransferMode::Delete, r"Y:\Filmer");
        assert_eq!(
            headline,
            r"2 of 3 files deleted from Y:\Filmer; 1 couldn't be deleted"
        );
        assert!(detail.contains("still there"));
    }

    fn request(mode: TransferMode, sources: &[&str], destination: &str) -> TransferRequest {
        TransferRequest {
            version: REQUEST_VERSION,
            mode,
            sources: sources.iter().map(|source| (*source).to_owned()).collect(),
            destination: destination.to_owned(),
        }
    }

    #[test]
    fn a_delete_request_needs_no_destination_but_a_copy_does() {
        assert!(validate_request(&request(TransferMode::Delete, &[r"Y:\a"], "")).is_ok());
        assert!(validate_request(&request(TransferMode::Copy, &[r"Y:\a"], "")).is_err());
        assert!(validate_request(&request(TransferMode::Delete, &[r"..\a"], "")).is_err());
    }

    #[test]
    fn a_delete_request_from_the_shell_parses_without_a_destination() {
        let parsed: TransferRequest = serde_json::from_str(
            r#"{"version":1,"mode":"delete","sources":["Y:\\Filmer\\a.mkv"]}"#,
        )
        .unwrap();

        assert_eq!(parsed.mode, TransferMode::Delete);
        assert_eq!(parsed.destination, "");
    }

    fn meta(history_id: i64) -> Meta {
        Meta {
            id: 0,
            history_id,
            connection: String::new(),
            direction: "delete",
            items: 1,
            first_item: "a.txt".to_owned(),
            destination: String::new(),
            started_at_ms: 0,
        }
    }

    async fn finished_delete(target: PathBuf) -> BatchHandle {
        let handle = batch::start_delete(
            DeleteRequest {
                targets: vec![Endpoint::Local(target)],
            },
            BatchOptions::default(),
        );
        handle.wait().await;
        handle
    }

    #[tokio::test]
    async fn the_window_closes_only_when_every_batch_finished_cleanly() {
        let manager = TransferManager::default();
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        manager.insert(finished_delete(file).await, meta(1));
        assert!(manager.all_clean());

        manager.insert(
            finished_delete(folder.path().join("missing.txt")).await,
            meta(2),
        );

        assert!(
            !manager.all_clean(),
            "a batch with problems keeps the window open"
        );
        manager.remove_finished();
        assert!(manager.views().is_empty());
        assert!(manager.all_clean(), "an empty list has nothing to show");
    }

    #[tokio::test]
    async fn the_log_finds_a_batch_by_its_row() {
        let manager = TransferManager::default();
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let transfer_id = manager.insert(finished_delete(file).await, meta(7));
        manager.insert(finished_delete(folder.path().join("none")).await, meta(0));

        let live = manager.live();

        assert_eq!(live.len(), 1, "batches without a log row are not listed");
        assert_eq!(live[&7].0, transfer_id);
        assert_eq!(live[&7].1.state, BatchState::Completed);
    }

    #[test]
    fn names_for_the_log_match_what_the_frontend_expects() {
        assert_eq!(mode_name(TransferMode::Delete), "delete");
        assert_eq!(mode_name(TransferMode::Move), "move");
        assert_eq!(
            state_name(BatchState::CompletedWithErrors),
            "completed_with_errors"
        );
        assert_eq!(
            state_name(BatchState::WaitingForAnswer),
            "waiting_for_answer"
        );
    }
}
