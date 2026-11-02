//! Batch copy/move between a local folder tree and any storage provider.
//!
//! A batch is planned up front (so totals are known), then run with a bounded number of
//! concurrent file transfers. Nothing is overwritten without an answer, and a move removes a
//! source only after its destination was written and, where the provider reports sizes,
//! verified.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use crate::concurrency::{Tuner, BYTES_PER_FILE};
use bifrost_common::RemotePath;
use bifrost_storage::{ReadRequest, StorageError, StorageProvider, WriteRequest};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::Instant,
};
use tokio_util::{io::ReaderStream, sync::CancellationToken};

const MAX_RECORDED_FAILURES: usize = 200;
/// The hard bound on files in flight, whatever the options or the tuner ask for.
const MAX_CONCURRENT: usize = 16;
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);
const RATE_WINDOW: Duration = Duration::from_secs(3);
const CHUNK_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    Copy,
    Move,
    /// Removes the chosen items; there is no destination.
    Delete,
}

#[derive(Clone)]
pub enum Endpoint {
    Local(PathBuf),
    Remote {
        provider: Arc<dyn StorageProvider>,
        path: RemotePath,
    },
}

pub struct BatchRequest {
    pub sources: Vec<Endpoint>,
    /// An existing folder that receives every source under its own name.
    pub destination: Endpoint,
    pub mode: TransferMode,
}

#[derive(Debug, Clone)]
pub struct BatchOptions {
    /// Files worked on at once when the batch starts (at most 16).
    pub max_concurrent: usize,
    /// When above `max_concurrent`, the batch measures its throughput and raises the number of
    /// files in flight, up to this many, for as long as that makes it faster.
    pub adaptive_ceiling: usize,
    /// How often the throughput is measured for `adaptive_ceiling`.
    pub tune_interval: Duration,
    /// Work through the files in path order, which keeps a spinning disk's head moving forward.
    pub ordered: bool,
    pub max_attempts: u32,
    pub retry_base_delay: Duration,
    /// Where remote-to-remote transfers stage data.
    pub temp_dir: PathBuf,
    /// Existing destination folders with more files than this are listed once instead of
    /// checking each file for a name clash.
    pub listing_threshold: usize,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            adaptive_ceiling: 0,
            tune_interval: Duration::from_millis(500),
            ordered: false,
            max_attempts: 3,
            retry_base_delay: Duration::from_secs(1),
            temp_dir: std::env::temp_dir(),
            listing_threshold: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum FailureReason {
    PermissionDenied,
    NotFound,
    AuthenticationFailed,
    Network(String),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemFailure {
    pub name: String,
    pub reason: FailureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Skip,
    Replace,
    KeepBoth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictPrompt {
    pub name: String,
    pub source_bytes: u64,
    pub destination_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchState {
    Planning,
    Running,
    Paused,
    WaitingForAnswer,
    Completed,
    CompletedWithErrors,
    Cancelled,
    Failed,
}

impl BatchState {
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::CompletedWithErrors | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatchSnapshot {
    pub state: BatchState,
    pub mode: TransferMode,
    pub total_files: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub done_bytes: u64,
    pub skipped_files: u64,
    pub failed_files: u64,
    /// The first failures, capped so a bad batch cannot grow without bound.
    pub failures: Vec<ItemFailure>,
    pub current: Vec<String>,
    pub bytes_per_second: u64,
    pub files_per_second: u64,
    pub seconds_left: Option<u64>,
    pub conflict: Option<ConflictPrompt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    Link,
}

#[derive(Debug, Clone, Copy)]
struct Found {
    kind: Kind,
    size: u64,
}

enum Stop {
    Cancelled,
    Failed(FailureReason),
}

impl From<FailureReason> for Stop {
    fn from(reason: FailureReason) -> Self {
        Self::Failed(reason)
    }
}

impl From<StorageError> for Stop {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::Cancelled => Self::Cancelled,
            error => Self::Failed(storage_reason(error)),
        }
    }
}

fn storage_reason(error: StorageError) -> FailureReason {
    match error {
        StorageError::PermissionDenied { .. } => FailureReason::PermissionDenied,
        StorageError::NotFound { .. } => FailureReason::NotFound,
        StorageError::AuthenticationFailed { .. } => FailureReason::AuthenticationFailed,
        StorageError::Network { message, .. } => FailureReason::Network(message),
        StorageError::Io(error) => FailureReason::Network(error.to_string()),
        StorageError::Provider { message, .. } => FailureReason::Other(message),
        StorageError::Unsupported { capability, .. } => {
            FailureReason::Other(format!("{capability} is not supported"))
        }
        StorageError::Cancelled => FailureReason::Other("cancelled".to_owned()),
    }
}

fn io_reason(error: std::io::Error) -> FailureReason {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => FailureReason::PermissionDenied,
        std::io::ErrorKind::NotFound => FailureReason::NotFound,
        _ => FailureReason::Other(error.to_string()),
    }
}

fn io_stop(error: std::io::Error) -> Stop {
    Stop::Failed(io_reason(error))
}

impl Endpoint {
    fn file_name(&self) -> String {
        match self {
            Self::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Self::Remote { path, .. } => path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned(),
        }
    }

    fn child(&self, name: &str) -> Result<Self, FailureReason> {
        match self {
            Self::Local(path) => Ok(Self::Local(path.join(name))),
            Self::Remote { provider, path } => path
                .join(name)
                .map(|path| Self::Remote {
                    provider: Arc::clone(provider),
                    path,
                })
                .map_err(|_| {
                    FailureReason::Other(format!("the name “{name}” is not allowed here"))
                }),
        }
    }

    fn sibling(&self, name: &str) -> Result<Self, FailureReason> {
        match self {
            Self::Local(path) => Ok(Self::Local(path.with_file_name(name))),
            Self::Remote { provider, path } => {
                let parent = path
                    .as_str()
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent);
                RemotePath::parse(parent)
                    .and_then(|parent| parent.join(name))
                    .map(|path| Self::Remote {
                        provider: Arc::clone(provider),
                        path,
                    })
                    .map_err(|_| {
                        FailureReason::Other(format!("the name “{name}” is not allowed here"))
                    })
            }
        }
    }

    async fn stat(&self) -> Result<Option<Found>, FailureReason> {
        match self {
            Self::Local(path) => match tokio::fs::symlink_metadata(path).await {
                Ok(metadata) => Ok(Some(Found {
                    kind: if metadata.file_type().is_symlink() {
                        Kind::Link
                    } else if metadata.is_dir() {
                        Kind::Directory
                    } else {
                        Kind::File
                    },
                    size: metadata.len(),
                })),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(io_reason(error)),
            },
            Self::Remote { provider, path } => match provider.stat(path).await {
                Ok(metadata) => Ok(Some(Found {
                    kind: if metadata.is_directory {
                        Kind::Directory
                    } else {
                        Kind::File
                    },
                    size: metadata.size_bytes.unwrap_or(0),
                })),
                Err(StorageError::NotFound { .. }) => Ok(None),
                Err(error) => Err(storage_reason(error)),
            },
        }
    }

    async fn list(&self) -> Result<Vec<(String, Found)>, FailureReason> {
        match self {
            Self::Local(path) => {
                // One blocking call for the whole folder: the entries already carry their type
                // and size on Windows, so asking for each one separately only adds hops.
                let path = path.clone();
                tokio::task::spawn_blocking(move || {
                    let mut found = Vec::new();
                    for entry in std::fs::read_dir(&path).map_err(io_reason)? {
                        let entry = entry.map_err(io_reason)?;
                        let file_type = entry.file_type().map_err(io_reason)?;
                        let size = if file_type.is_file() {
                            entry.metadata().map_err(io_reason)?.len()
                        } else {
                            0
                        };
                        found.push((
                            entry.file_name().to_string_lossy().into_owned(),
                            Found {
                                kind: if file_type.is_symlink() {
                                    Kind::Link
                                } else if file_type.is_dir() {
                                    Kind::Directory
                                } else {
                                    Kind::File
                                },
                                size,
                            },
                        ));
                    }
                    Ok(found)
                })
                .await
                .map_err(|error| FailureReason::Other(error.to_string()))?
            }
            Self::Remote { provider, path } => {
                let mut found = Vec::new();
                let mut cursor = None;
                loop {
                    let page = provider
                        .list(path, cursor.as_deref())
                        .await
                        .map_err(storage_reason)?;
                    for entry in page.entries {
                        let name = entry
                            .metadata
                            .path
                            .as_str()
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        found.push((
                            name,
                            Found {
                                kind: if entry.metadata.is_directory {
                                    Kind::Directory
                                } else {
                                    Kind::File
                                },
                                size: entry.metadata.size_bytes.unwrap_or(0),
                            },
                        ));
                    }
                    cursor = page.next_cursor;
                    if cursor.is_none() {
                        return Ok(found);
                    }
                }
            }
        }
    }

    async fn make_dir(&self) -> Result<(), FailureReason> {
        match self {
            Self::Local(path) => tokio::fs::create_dir(path).await.map_err(io_reason),
            Self::Remote { provider, path } => provider
                .create_directory(path)
                .await
                .map_err(storage_reason),
        }
    }

    async fn remove_file(&self) -> Result<(), FailureReason> {
        match self {
            Self::Local(path) => tokio::fs::remove_file(path).await.map_err(io_reason),
            Self::Remote { provider, path } => provider.delete(path).await.map_err(storage_reason),
        }
    }

    /// Removes a folder only when it is empty, because some providers delete recursively.
    async fn remove_empty_dir(&self) -> Result<(), FailureReason> {
        match self {
            Self::Local(path) => match tokio::fs::remove_dir(path).await {
                Ok(()) => Ok(()),
                Err(error) if is_not_empty(&error) => Ok(()),
                Err(error) => Err(io_reason(error)),
            },
            Self::Remote { provider, path } => {
                if !self.list().await?.is_empty() {
                    return Ok(());
                }
                provider.delete(path).await.map_err(storage_reason)
            }
        }
    }
}

fn is_not_empty(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::DirectoryNotEmpty
}

struct FileJob {
    source: Endpoint,
    destination: Endpoint,
    /// Path shown to the user, relative to the chosen source.
    relative: String,
    size: u64,
    dir: usize,
}

struct DirPlan {
    endpoint: Endpoint,
    parent: Option<usize>,
    existing: bool,
    ok: bool,
    jobs: usize,
    listing: Option<HashMap<String, Found>>,
}

#[derive(Default)]
struct Plan {
    jobs: Vec<FileJob>,
    dirs: Vec<Option<DirPlan>>,
    /// Source folders in discovery order, so removing them in reverse removes children first.
    source_dirs: Vec<Endpoint>,
}

#[derive(Default)]
struct Inner {
    planning: bool,
    finished: Option<BatchState>,
    total_files: u64,
    total_bytes: u64,
    failures: Vec<ItemFailure>,
    current: Vec<String>,
    conflict: Option<ConflictPrompt>,
    answer: Option<oneshot::Sender<(Resolution, bool)>>,
    policy: Option<Resolution>,
}

struct Shared {
    mode: TransferMode,
    cancel: CancellationToken,
    paused: watch::Sender<bool>,
    done_bytes: AtomicU64,
    done_files: AtomicUsize,
    skipped_files: AtomicUsize,
    failed_files: AtomicUsize,
    bytes_per_second: AtomicU64,
    files_per_second: AtomicU64,
    inner: Mutex<Inner>,
    conflict_gate: tokio::sync::Mutex<()>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> BatchSnapshot {
        let inner = self.lock();
        let done = self.done_bytes.load(Ordering::Relaxed);
        let done_bytes = if inner.total_bytes > 0 {
            done.min(inner.total_bytes)
        } else {
            done
        };
        let rate = self.bytes_per_second.load(Ordering::Relaxed);
        let file_rate = self.files_per_second.load(Ordering::Relaxed);
        let state = if let Some(state) = inner.finished {
            state
        } else if inner.conflict.is_some() {
            BatchState::WaitingForAnswer
        } else if *self.paused.borrow() {
            BatchState::Paused
        } else if inner.planning {
            BatchState::Planning
        } else {
            BatchState::Running
        };
        let remaining = inner.total_bytes.saturating_sub(done_bytes);
        let remaining_files = inner
            .total_files
            .saturating_sub(self.done_files.load(Ordering::Relaxed) as u64);
        BatchSnapshot {
            state,
            mode: self.mode,
            total_files: inner.total_files,
            total_bytes: inner.total_bytes,
            done_files: self.done_files.load(Ordering::Relaxed) as u64,
            done_bytes,
            skipped_files: self.skipped_files.load(Ordering::Relaxed) as u64,
            failed_files: self.failed_files.load(Ordering::Relaxed) as u64,
            failures: inner.failures.clone(),
            current: inner.current.clone(),
            bytes_per_second: rate,
            files_per_second: file_rate,
            // Deleting moves no data, so its estimate follows the files instead.
            seconds_left: (state == BatchState::Running)
                .then(|| match self.mode {
                    TransferMode::Delete => (file_rate > 0).then(|| remaining_files / file_rate),
                    _ => (rate > 0).then(|| remaining / rate),
                })
                .flatten(),
            conflict: inner.conflict.clone(),
        }
    }

    fn record_failure(&self, name: &str, reason: FailureReason) {
        self.failed_files.fetch_add(1, Ordering::Relaxed);
        self.note_failure(name, reason);
    }

    fn note_failure(&self, name: &str, reason: FailureReason) {
        let mut inner = self.lock();
        if inner.failures.len() < MAX_RECORDED_FAILURES {
            inner.failures.push(ItemFailure {
                name: name.to_owned(),
                reason,
            });
        }
    }

    /// Waits while paused; returns false when the batch was cancelled instead.
    async fn proceed(&self) -> bool {
        let mut paused = self.paused.subscribe();
        loop {
            if self.cancel.is_cancelled() {
                return false;
            }
            if !*paused.borrow() {
                return true;
            }
            tokio::select! {
                _ = paused.changed() => {}
                _ = self.cancel.cancelled() => return false,
            }
        }
    }

    /// Asks once at a time; `None` means the batch was cancelled while waiting.
    async fn resolve_conflict(&self, prompt: ConflictPrompt) -> Option<Resolution> {
        if let Some(policy) = self.lock().policy {
            return Some(policy);
        }
        let _gate = self.conflict_gate.lock().await;
        if let Some(policy) = self.lock().policy {
            return Some(policy);
        }
        let (answer, receiver) = oneshot::channel();
        {
            let mut inner = self.lock();
            inner.conflict = Some(prompt);
            inner.answer = Some(answer);
        }
        let reply = tokio::select! {
            reply = receiver => reply.ok(),
            _ = self.cancel.cancelled() => None,
        };
        let mut inner = self.lock();
        inner.conflict = None;
        inner.answer = None;
        let (resolution, apply_to_all) = reply?;
        if apply_to_all {
            inner.policy = Some(resolution);
        }
        Some(resolution)
    }
}

/// Counts the bytes of one attempt so a retry can take them back.
#[derive(Clone)]
struct Progress {
    shared: Arc<Shared>,
    counted: Arc<AtomicU64>,
    seen: Arc<AtomicU64>,
}

impl Progress {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            counted: Arc::new(AtomicU64::new(0)),
            seen: Arc::new(AtomicU64::new(0)),
        }
    }

    /// `halved` splits a two-phase transfer (download then upload) into equal halves.
    fn add(&self, bytes: usize, halved: bool) {
        let bytes = bytes as u64;
        let delta = if halved {
            let before = self.seen.fetch_add(bytes, Ordering::Relaxed);
            (before + bytes) / 2 - before / 2
        } else {
            bytes
        };
        self.counted.fetch_add(delta, Ordering::Relaxed);
        self.shared.done_bytes.fetch_add(delta, Ordering::Relaxed);
    }

    fn reset(&self) {
        let counted = self.counted.swap(0, Ordering::Relaxed);
        self.seen.store(0, Ordering::Relaxed);
        self.shared.done_bytes.fetch_sub(counted, Ordering::Relaxed);
    }

    fn complete(&self, size: u64) {
        let counted = self.counted.swap(size, Ordering::Relaxed);
        if size >= counted {
            self.shared
                .done_bytes
                .fetch_add(size - counted, Ordering::Relaxed);
        } else {
            self.shared
                .done_bytes
                .fetch_sub(counted - size, Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
pub struct BatchHandle {
    shared: Arc<Shared>,
    snapshots: watch::Receiver<BatchSnapshot>,
}

impl BatchHandle {
    pub fn snapshot(&self) -> BatchSnapshot {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<BatchSnapshot> {
        self.snapshots.clone()
    }

    /// In-flight files finish; no new file starts until `resume`.
    pub fn pause(&self) {
        self.shared.paused.send_replace(true);
    }

    pub fn resume(&self) {
        self.shared.paused.send_replace(false);
    }

    pub fn cancel(&self) {
        self.shared.cancel.cancel();
    }

    /// Answers the pending name clash, if any.
    pub fn answer_conflict(&self, resolution: Resolution, apply_to_all: bool) {
        if let Some(answer) = self.shared.lock().answer.take() {
            let _ = answer.send((resolution, apply_to_all));
        }
    }

    pub async fn wait(&self) -> BatchSnapshot {
        let mut snapshots = self.snapshots.clone();
        let finished = snapshots
            .wait_for(|snapshot| snapshot.state.is_final())
            .await
            .map(|snapshot| snapshot.clone());
        finished.unwrap_or_else(|_| self.snapshot())
    }
}

/// Starts a batch on the current Tokio runtime.
pub fn start(request: BatchRequest, options: BatchOptions) -> BatchHandle {
    let (shared, handle) = begin(request.mode);
    tokio::spawn(execute(shared, request, options));
    handle
}

/// What to remove for [`start_delete`].
pub struct DeleteRequest {
    pub targets: Vec<Endpoint>,
}

/// Starts deleting `request.targets` (folders with everything inside them) on the current Tokio
/// runtime, many files at once. Files go first, then the emptied folders from the deepest
/// level up, and a folder that still holds anything is left alone.
pub fn start_delete(request: DeleteRequest, options: BatchOptions) -> BatchHandle {
    let (shared, handle) = begin(TransferMode::Delete);
    tokio::spawn(execute_delete(shared, request, options));
    handle
}

fn begin(mode: TransferMode) -> (Arc<Shared>, BatchHandle) {
    let (paused, _) = watch::channel(false);
    let shared = Arc::new(Shared {
        mode,
        cancel: CancellationToken::new(),
        paused,
        done_bytes: AtomicU64::new(0),
        done_files: AtomicUsize::new(0),
        skipped_files: AtomicUsize::new(0),
        failed_files: AtomicUsize::new(0),
        bytes_per_second: AtomicU64::new(0),
        files_per_second: AtomicU64::new(0),
        inner: Mutex::new(Inner {
            planning: true,
            ..Default::default()
        }),
        conflict_gate: tokio::sync::Mutex::new(()),
    });
    let (sender, snapshots) = watch::channel(shared.snapshot());
    tokio::spawn(publish_snapshots(Arc::clone(&shared), sender));
    let handle = BatchHandle {
        shared: Arc::clone(&shared),
        snapshots,
    };
    (shared, handle)
}

async fn publish_snapshots(shared: Arc<Shared>, sender: watch::Sender<BatchSnapshot>) {
    let mut ticks = tokio::time::interval(SNAPSHOT_INTERVAL);
    let mut samples: VecDeque<(Instant, u64, u64)> = VecDeque::new();
    loop {
        ticks.tick().await;
        let now = Instant::now();
        let done = shared.done_bytes.load(Ordering::Relaxed);
        let files = (shared.done_files.load(Ordering::Relaxed)
            + shared.skipped_files.load(Ordering::Relaxed)) as u64;
        samples.push_back((now, done, files));
        while samples
            .front()
            .is_some_and(|(at, _, _)| now.duration_since(*at) > RATE_WINDOW)
        {
            samples.pop_front();
        }
        if let Some((first_at, first_done, first_files)) = samples.front() {
            let elapsed = now.duration_since(*first_at).as_secs_f64();
            let (rate, file_rate) = if elapsed > 0.0 {
                (
                    (done.saturating_sub(*first_done) as f64 / elapsed) as u64,
                    (files.saturating_sub(*first_files) as f64 / elapsed) as u64,
                )
            } else {
                (0, 0)
            };
            shared.bytes_per_second.store(rate, Ordering::Relaxed);
            shared.files_per_second.store(file_rate, Ordering::Relaxed);
        }
        let snapshot = shared.snapshot();
        let finished = snapshot.state.is_final();
        if sender.send(snapshot).is_err() || finished {
            return;
        }
    }
}

async fn execute(shared: Arc<Shared>, request: BatchRequest, options: BatchOptions) {
    let state = run(&shared, request, &options).await;
    conclude(&shared, state);
}

async fn execute_delete(shared: Arc<Shared>, request: DeleteRequest, options: BatchOptions) {
    let state = run_delete(&shared, request, &options).await;
    conclude(&shared, state);
}

fn conclude(shared: &Shared, state: BatchState) {
    {
        let mut inner = shared.lock();
        inner.finished = Some(state);
        inner.current.clear();
        inner.conflict = None;
    }
    shared.bytes_per_second.store(0, Ordering::Relaxed);
    shared.files_per_second.store(0, Ordering::Relaxed);
}

async fn run(shared: &Arc<Shared>, request: BatchRequest, options: &BatchOptions) -> BatchState {
    match request.destination.stat().await {
        Ok(Some(Found {
            kind: Kind::Directory,
            ..
        })) => {}
        Ok(_) => {
            shared.note_failure(
                &request.destination.file_name(),
                FailureReason::Other("the destination is not a folder".to_owned()),
            );
            return BatchState::Failed;
        }
        Err(reason) => {
            shared.note_failure(&request.destination.file_name(), reason);
            return BatchState::Failed;
        }
    }

    let plan = build_plan(shared, &request).await;
    {
        let mut inner = shared.lock();
        inner.planning = false;
        inner.total_files = plan.jobs.len() as u64;
        inner.total_bytes = plan.jobs.iter().map(|job| job.size).sum();
    }
    if shared.cancel.is_cancelled() {
        return BatchState::Cancelled;
    }

    let mut plan = plan;
    prepare_folders(shared, &mut plan, options).await;
    let source_dirs = std::mem::take(&mut plan.source_dirs);
    let mut jobs = std::mem::take(&mut plan.jobs);
    if options.ordered {
        jobs.sort_by(|first, second| first.relative.cmp(&second.relative));
    }
    let semaphore = Arc::new(Semaphore::new(
        options.max_concurrent.clamp(1, MAX_CONCURRENT),
    ));
    let tuner = spawn_tuner(shared, &semaphore, options);
    let context = Arc::new(Context {
        shared: Arc::clone(shared),
        dirs: plan.dirs.into_iter().map(Option::unwrap).collect(),
        semaphore,
        options: options.clone(),
    });

    let mut running = JoinSet::new();
    for job in jobs {
        if !shared.proceed().await {
            break;
        }
        let Ok(permit) = Arc::clone(&context.semaphore).acquire_owned().await else {
            break;
        };
        let context = Arc::clone(&context);
        running.spawn(async move { context.process(job, permit).await });
    }
    while running.join_next().await.is_some() {}
    if let Some(tuner) = tuner {
        tuner.abort();
    }

    if shared.cancel.is_cancelled() {
        return BatchState::Cancelled;
    }
    let clean = shared.failed_files.load(Ordering::Relaxed) == 0
        && shared.skipped_files.load(Ordering::Relaxed) == 0;
    if request.mode == TransferMode::Move && clean {
        for directory in source_dirs.iter().rev() {
            if let Err(reason) = directory.remove_empty_dir().await {
                shared.note_failure(&directory.file_name(), reason);
                shared.failed_files.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    if shared.failed_files.load(Ordering::Relaxed) > 0 {
        BatchState::CompletedWithErrors
    } else {
        BatchState::Completed
    }
}

async fn build_plan(shared: &Arc<Shared>, request: &BatchRequest) -> Plan {
    let mut plan = Plan::default();
    plan.dirs.push(Some(DirPlan {
        endpoint: request.destination.clone(),
        parent: None,
        existing: true,
        ok: true,
        jobs: 0,
        listing: None,
    }));
    for source in &request.sources {
        if shared.cancel.is_cancelled() {
            break;
        }
        let name = source.file_name();
        let found = match source.stat().await {
            Ok(Some(found)) => found,
            Ok(None) => {
                shared.record_failure(&name, FailureReason::NotFound);
                continue;
            }
            Err(reason) => {
                shared.record_failure(&name, reason);
                continue;
            }
        };
        let destination = match request.destination.child(&name) {
            Ok(destination) => destination,
            Err(reason) => {
                shared.record_failure(&name, reason);
                continue;
            }
        };
        match found.kind {
            Kind::Link => {
                shared.skipped_files.fetch_add(1, Ordering::Relaxed);
            }
            Kind::File => push_job(&mut plan, source.clone(), destination, name, found.size, 0),
            Kind::Directory => {
                plan_directory(shared, &mut plan, source.clone(), destination, name, 0).await
            }
        }
    }
    plan
}

fn push_job(
    plan: &mut Plan,
    source: Endpoint,
    destination: Endpoint,
    relative: String,
    size: u64,
    dir: usize,
) {
    if let Some(Some(directory)) = plan.dirs.get_mut(dir) {
        directory.jobs += 1;
    }
    plan.jobs.push(FileJob {
        source,
        destination,
        relative,
        size,
        dir,
    });
}

async fn plan_directory(
    shared: &Arc<Shared>,
    plan: &mut Plan,
    source: Endpoint,
    destination: Endpoint,
    relative: String,
    parent: usize,
) {
    let mut stack = vec![(source, destination, relative, parent)];
    while let Some((source, destination, relative, parent)) = stack.pop() {
        if shared.cancel.is_cancelled() {
            return;
        }
        let index = plan.dirs.len();
        plan.dirs.push(Some(DirPlan {
            endpoint: destination.clone(),
            parent: Some(parent),
            existing: false,
            ok: false,
            jobs: 0,
            listing: None,
        }));
        plan.source_dirs.push(source.clone());
        let entries = match source.list().await {
            Ok(entries) => entries,
            Err(reason) => {
                shared.record_failure(&relative, reason);
                continue;
            }
        };
        let mut children = Vec::new();
        for (name, found) in entries {
            let shown = format!("{relative}/{name}");
            let (child_source, child_destination) =
                match (source.child(&name), destination.child(&name)) {
                    (Ok(source), Ok(destination)) => (source, destination),
                    (Err(reason), _) | (_, Err(reason)) => {
                        shared.record_failure(&shown, reason);
                        continue;
                    }
                };
            match found.kind {
                Kind::Link => {
                    shared.skipped_files.fetch_add(1, Ordering::Relaxed);
                }
                Kind::File => push_job(
                    plan,
                    child_source,
                    child_destination,
                    shown,
                    found.size,
                    index,
                ),
                Kind::Directory => children.push((child_source, child_destination, shown, index)),
            }
        }
        // Pushed in reverse so folders are visited in listing order.
        stack.extend(children.into_iter().rev());
    }
}

/// Creates the destination folders, parents first, and lists busy existing ones once.
async fn prepare_folders(shared: &Arc<Shared>, plan: &mut Plan, options: &BatchOptions) {
    for index in 1..plan.dirs.len() {
        if shared.cancel.is_cancelled() {
            return;
        }
        let (endpoint, parent) = {
            let directory = plan.dirs[index].as_ref().unwrap();
            (directory.endpoint.clone(), directory.parent.unwrap())
        };
        let (parent_ok, parent_existing) = {
            let parent = plan.dirs[parent].as_ref().unwrap();
            (parent.ok, parent.existing)
        };
        let name = endpoint.file_name();
        let mut existing = false;
        let mut ok = false;
        if parent_ok {
            let found = if parent_existing {
                endpoint.stat().await
            } else {
                Ok(None)
            };
            match found {
                Ok(Some(Found {
                    kind: Kind::Directory,
                    ..
                })) => {
                    existing = true;
                    ok = true;
                }
                Ok(Some(_)) => shared.note_failure(
                    &name,
                    FailureReason::Other("a file with this name already exists".to_owned()),
                ),
                Ok(None) => match endpoint.make_dir().await {
                    Ok(()) => ok = true,
                    Err(reason) => shared.note_failure(&name, reason),
                },
                Err(reason) => shared.note_failure(&name, reason),
            }
        }
        let directory = plan.dirs[index].as_mut().unwrap();
        directory.existing = existing;
        directory.ok = ok;
    }
    for directory in plan.dirs.iter_mut().flatten() {
        if directory.ok && directory.existing && directory.jobs > options.listing_threshold {
            directory.listing = directory
                .endpoint
                .list()
                .await
                .ok()
                .map(|entries| entries.into_iter().collect());
        }
    }
}

/// Work done so far, in the unit [`Tuner`] compares: bytes plus a fixed cost per file.
fn work_done(shared: &Shared) -> u64 {
    let files = shared.done_files.load(Ordering::Relaxed)
        + shared.skipped_files.load(Ordering::Relaxed)
        + shared.failed_files.load(Ordering::Relaxed);
    shared
        .done_bytes
        .load(Ordering::Relaxed)
        .saturating_add(files as u64 * BYTES_PER_FILE)
}

/// Adjusts how many files are in flight to the measured throughput, when the options allow
/// more than the starting number. Returns `None` for a fixed amount of parallelism.
fn spawn_tuner(
    shared: &Arc<Shared>,
    semaphore: &Arc<Semaphore>,
    options: &BatchOptions,
) -> Option<tokio::task::JoinHandle<()>> {
    let start = options.max_concurrent.clamp(1, MAX_CONCURRENT);
    let ceiling = options.adaptive_ceiling.clamp(start, MAX_CONCURRENT);
    if ceiling == start {
        return None;
    }
    let shared = Arc::clone(shared);
    let semaphore = Arc::clone(semaphore);
    let interval = options.tune_interval;
    Some(tokio::spawn(async move {
        let mut tuner = Tuner::new(start, ceiling);
        let mut current = start;
        let mut ticks = tokio::time::interval(interval);
        ticks.tick().await;
        let mut last = (Instant::now(), work_done(&shared));
        loop {
            ticks.tick().await;
            let now = Instant::now();
            let done = work_done(&shared);
            let seconds = now.duration_since(last.0).as_secs_f64().max(f64::EPSILON);
            let rate = done.saturating_sub(last.1) as f64 / seconds;
            last = (now, done);
            if *shared.paused.borrow() || shared.lock().conflict.is_some() {
                continue;
            }
            if let Some(target) = tuner.observe(rate) {
                resize(&semaphore, current, target);
                current = target;
            }
            if tuner.is_settled() {
                return;
            }
        }
    }))
}

/// Changes the number of permits from `current` to `target`. Removing permits waits for files
/// to finish, so a lower setting takes effect as work completes.
fn resize(semaphore: &Arc<Semaphore>, current: usize, target: usize) {
    if target > current {
        semaphore.add_permits(target - current);
    }
    for _ in target..current {
        let semaphore = Arc::clone(semaphore);
        tokio::spawn(async move {
            if let Ok(permit) = semaphore.acquire_owned().await {
                permit.forget();
            }
        });
    }
}

struct Doomed {
    endpoint: Endpoint,
    relative: String,
    size: u64,
    link: bool,
}

struct DoomedDirectory {
    endpoint: Endpoint,
    relative: String,
    depth: usize,
}

async fn run_delete(
    shared: &Arc<Shared>,
    request: DeleteRequest,
    options: &BatchOptions,
) -> BatchState {
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for target in &request.targets {
        if shared.cancel.is_cancelled() {
            break;
        }
        let name = target.file_name();
        match target.stat().await {
            Ok(Some(Found {
                kind: Kind::Directory,
                ..
            })) => plan_delete(shared, target.clone(), name, &mut files, &mut directories).await,
            Ok(Some(found)) => files.push(Doomed {
                endpoint: target.clone(),
                relative: name,
                size: found.size,
                link: found.kind == Kind::Link,
            }),
            Ok(None) => shared.record_failure(&name, FailureReason::NotFound),
            Err(reason) => shared.record_failure(&name, reason),
        }
    }
    {
        let mut inner = shared.lock();
        inner.planning = false;
        inner.total_files = files.len() as u64;
        inner.total_bytes = files.iter().map(|file| file.size).sum();
    }
    if shared.cancel.is_cancelled() {
        return BatchState::Cancelled;
    }
    if options.ordered {
        files.sort_by(|first, second| first.relative.cmp(&second.relative));
    }

    let semaphore = Arc::new(Semaphore::new(
        options.max_concurrent.clamp(1, MAX_CONCURRENT),
    ));
    let tuner = spawn_tuner(shared, &semaphore, options);
    let mut running = JoinSet::new();
    for doomed in files {
        if !shared.proceed().await {
            break;
        }
        let Ok(permit) = Arc::clone(&semaphore).acquire_owned().await else {
            break;
        };
        let shared = Arc::clone(shared);
        let (attempts, delay) = (options.max_attempts, options.retry_base_delay);
        running.spawn(async move {
            delete_one(&shared, doomed, attempts, delay).await;
            drop(permit);
        });
    }
    while running.join_next().await.is_some() {}
    if let Some(tuner) = tuner {
        tuner.abort();
    }

    // Folders last, deepest level first, so each is empty by the time it is removed.
    let deepest = directories
        .iter()
        .map(|directory| directory.depth)
        .max()
        .unwrap_or(0);
    'levels: for depth in (0..=deepest).rev() {
        for directory in directories
            .iter()
            .filter(|directory| directory.depth == depth)
        {
            if !shared.proceed().await {
                break 'levels;
            }
            let Ok(permit) = Arc::clone(&semaphore).acquire_owned().await else {
                break 'levels;
            };
            let shared = Arc::clone(shared);
            let endpoint = directory.endpoint.clone();
            let relative = directory.relative.clone();
            running.spawn(async move {
                if let Err(reason) = endpoint.remove_empty_dir().await {
                    shared.record_failure(&relative, reason);
                }
                drop(permit);
            });
        }
        while running.join_next().await.is_some() {}
    }
    while running.join_next().await.is_some() {}

    if shared.cancel.is_cancelled() {
        BatchState::Cancelled
    } else if shared.failed_files.load(Ordering::Relaxed) > 0 {
        BatchState::CompletedWithErrors
    } else {
        BatchState::Completed
    }
}

async fn plan_delete(
    shared: &Arc<Shared>,
    root: Endpoint,
    relative: String,
    files: &mut Vec<Doomed>,
    directories: &mut Vec<DoomedDirectory>,
) {
    let mut stack = vec![(root, relative, 0usize)];
    while let Some((endpoint, relative, depth)) = stack.pop() {
        if shared.cancel.is_cancelled() {
            return;
        }
        directories.push(DoomedDirectory {
            endpoint: endpoint.clone(),
            relative: relative.clone(),
            depth,
        });
        let entries = match endpoint.list().await {
            Ok(entries) => entries,
            Err(reason) => {
                shared.record_failure(&relative, reason);
                continue;
            }
        };
        for (name, found) in entries {
            let shown = format!("{relative}/{name}");
            let child = match endpoint.child(&name) {
                Ok(child) => child,
                Err(reason) => {
                    shared.record_failure(&shown, reason);
                    continue;
                }
            };
            match found.kind {
                Kind::Directory => stack.push((child, shown, depth + 1)),
                Kind::File | Kind::Link => files.push(Doomed {
                    endpoint: child,
                    relative: shown,
                    size: found.size,
                    link: found.kind == Kind::Link,
                }),
            }
        }
    }
}

async fn remove_entry(doomed: &Doomed) -> Result<(), FailureReason> {
    match &doomed.endpoint {
        // A link to a folder is removed like a folder, without touching what it points to.
        Endpoint::Local(path) if doomed.link => match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(_) => tokio::fs::remove_dir(path).await.map_err(io_reason),
        },
        endpoint => endpoint.remove_file().await,
    }
}

async fn delete_one(shared: &Arc<Shared>, doomed: Doomed, max_attempts: u32, base: Duration) {
    shared.lock().current.push(doomed.relative.clone());
    let mut attempt = 1;
    let outcome = loop {
        match remove_entry(&doomed).await {
            // Already gone is what the user wanted.
            Ok(()) | Err(FailureReason::NotFound) => break Some(Ok(())),
            Err(FailureReason::Network(_)) if attempt < max_attempts => {
                let delay = base * 2u32.pow(attempt - 1);
                attempt += 1;
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shared.cancel.cancelled() => break None,
                }
            }
            Err(reason) => break Some(Err(reason)),
        }
    };
    {
        let mut inner = shared.lock();
        if let Some(position) = inner
            .current
            .iter()
            .position(|name| *name == doomed.relative)
        {
            inner.current.remove(position);
        }
    }
    shared.done_bytes.fetch_add(doomed.size, Ordering::Relaxed);
    match outcome {
        Some(Ok(())) => {
            shared.done_files.fetch_add(1, Ordering::Relaxed);
        }
        Some(Err(reason)) => shared.record_failure(&doomed.relative, reason),
        None => {}
    }
}

struct Context {
    shared: Arc<Shared>,
    dirs: Vec<DirPlan>,
    semaphore: Arc<Semaphore>,
    options: BatchOptions,
}

enum Handled {
    Done,
    Skipped,
    /// The destination folder could not be created; that was reported once for the folder.
    FolderFailed,
}

impl Context {
    async fn process(self: Arc<Self>, job: FileJob, permit: OwnedSemaphorePermit) {
        self.shared.lock().current.push(job.relative.clone());
        let result = self.handle(&job, permit).await;
        {
            let mut inner = self.shared.lock();
            if let Some(position) = inner.current.iter().position(|name| *name == job.relative) {
                inner.current.remove(position);
            }
        }
        match result {
            Ok(Handled::Done) => {
                self.shared.done_files.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Handled::Skipped) => {
                self.shared.skipped_files.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Handled::FolderFailed) => {
                self.shared.failed_files.fetch_add(1, Ordering::Relaxed);
            }
            Err(Stop::Cancelled) => {}
            Err(Stop::Failed(reason)) => self.shared.record_failure(&job.relative, reason),
        }
    }

    async fn handle(&self, job: &FileJob, permit: OwnedSemaphorePermit) -> Result<Handled, Stop> {
        let directory = &self.dirs[job.dir];
        if !directory.ok {
            // The folder's own failure was already reported once.
            self.shared
                .done_bytes
                .fetch_add(job.size, Ordering::Relaxed);
            return Ok(Handled::FolderFailed);
        }
        let name = job.destination.file_name();
        let existing = match &directory.listing {
            Some(listing) => listing.get(&name).copied(),
            None if directory.existing => job.destination.stat().await?,
            None => None,
        };
        let mut permit = Some(permit);
        let mut destination = job.destination.clone();
        let mut replace = false;
        if let Some(found) = existing {
            if found.kind != Kind::File {
                return Err(FailureReason::Other(
                    "a folder with this name already exists".to_owned(),
                )
                .into());
            }
            // Waiting for an answer must not hold up the other transfers.
            drop(permit.take());
            let resolution = self
                .shared
                .resolve_conflict(ConflictPrompt {
                    name: job.relative.clone(),
                    source_bytes: job.size,
                    destination_bytes: found.size,
                })
                .await
                .ok_or(Stop::Cancelled)?;
            permit = Some(
                Arc::clone(&self.semaphore)
                    .acquire_owned()
                    .await
                    .map_err(|_| Stop::Cancelled)?,
            );
            match resolution {
                Resolution::Skip => {
                    self.shared
                        .done_bytes
                        .fetch_add(job.size, Ordering::Relaxed);
                    return Ok(Handled::Skipped);
                }
                Resolution::Replace => replace = true,
                Resolution::KeepBoth => destination = unique_name(&job.destination).await?,
            }
        }
        let result = self.copy_with_retry(job, &destination, replace).await;
        drop(permit);
        result.map(|()| Handled::Done)
    }

    async fn copy_with_retry(
        &self,
        job: &FileJob,
        destination: &Endpoint,
        replace: bool,
    ) -> Result<(), Stop> {
        let progress = Progress::new(Arc::clone(&self.shared));
        let mut attempt = 0;
        loop {
            attempt += 1;
            if self.shared.cancel.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            match self.copy_once(job, destination, replace, &progress).await {
                Ok(()) => {
                    progress.complete(job.size);
                    return Ok(());
                }
                Err(Stop::Cancelled) => return Err(Stop::Cancelled),
                Err(Stop::Failed(reason)) => {
                    let retry = matches!(reason, FailureReason::Network(_))
                        && attempt < self.options.max_attempts;
                    if !retry {
                        progress.complete(job.size);
                        return Err(Stop::Failed(reason));
                    }
                    progress.reset();
                    let delay = self.options.retry_base_delay * 2u32.pow(attempt - 1);
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = self.shared.cancel.cancelled() => return Err(Stop::Cancelled),
                    }
                }
            }
        }
    }

    async fn copy_once(
        &self,
        job: &FileJob,
        destination: &Endpoint,
        replace: bool,
        progress: &Progress,
    ) -> Result<(), Stop> {
        let moving = self.shared.mode == TransferMode::Move;
        let cancel = &self.shared.cancel;
        match (&job.source, destination) {
            (Endpoint::Local(source), Endpoint::Remote { provider, path }) => {
                let written = upload_local(source, provider, path, progress, cancel, false).await?;
                if moving {
                    verify_remote(provider, path, written).await?;
                }
            }
            (Endpoint::Remote { provider, path }, Endpoint::Local(target)) => {
                download_remote(
                    provider, path, target, job.size, replace, progress, cancel, moving,
                )
                .await?;
            }
            (
                Endpoint::Remote {
                    provider: from_provider,
                    path: from,
                },
                Endpoint::Remote {
                    provider: to_provider,
                    path: to,
                },
            ) => {
                if Arc::ptr_eq(from_provider, to_provider) {
                    let server_side = if moving {
                        if replace {
                            from_provider.replace(from, to).await
                        } else {
                            from_provider.rename(from, to).await
                        }
                    } else if replace {
                        Err(StorageError::Unsupported {
                            provider: from_provider.kind(),
                            capability: "server_side_copy".to_owned(),
                        })
                    } else {
                        from_provider.copy(from, to).await
                    };
                    match server_side {
                        Ok(()) => {
                            progress.complete(job.size);
                            return Ok(());
                        }
                        Err(StorageError::Unsupported { .. }) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                let staged = tokio::task::spawn_blocking({
                    let directory = self.options.temp_dir.clone();
                    move || tempfile::NamedTempFile::new_in(directory)
                })
                .await
                .map_err(|error| Stop::Failed(FailureReason::Other(error.to_string())))?
                .map_err(io_stop)?
                .into_temp_path();
                let received = download_to_file(
                    from_provider,
                    from,
                    &staged,
                    job.size,
                    progress,
                    cancel,
                    true,
                    false,
                )
                .await?;
                let written =
                    upload_local(&staged, to_provider, to, progress, cancel, true).await?;
                if written != received {
                    return Err(FailureReason::Other(
                        "the staged copy changed during the transfer".to_owned(),
                    )
                    .into());
                }
                if moving {
                    verify_remote(to_provider, to, written).await?;
                }
            }
            (Endpoint::Local(source), Endpoint::Local(target)) => {
                match transfer_local(source, target, moving, progress, cancel).await? {
                    // Pasting a file onto itself leaves nothing to copy, and the "original"
                    // must not be removed afterwards because it is the result. A rename has
                    // already taken the original with it.
                    LocalOutcome::Unchanged | LocalOutcome::Renamed => {
                        progress.complete(job.size);
                        return Ok(());
                    }
                    LocalOutcome::Copied => {}
                }
            }
        }
        if moving {
            job.source.remove_file().await.map_err(|reason| {
                Stop::Failed(FailureReason::Other(format!(
                    "copied, but the original could not be removed ({})",
                    reason_text(&reason)
                )))
            })?;
        }
        Ok(())
    }
}

fn reason_text(reason: &FailureReason) -> String {
    match reason {
        FailureReason::PermissionDenied => "permission denied".to_owned(),
        FailureReason::NotFound => "not found".to_owned(),
        FailureReason::AuthenticationFailed => "sign-in rejected".to_owned(),
        FailureReason::Network(detail) | FailureReason::Other(detail) => detail.clone(),
    }
}

async fn unique_name(destination: &Endpoint) -> Result<Endpoint, Stop> {
    let name = destination.file_name();
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    for number in 2..10_000 {
        let candidate = destination.sibling(&format!("{stem} ({number}){extension}"))?;
        if candidate.stat().await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(FailureReason::Other("no free name for the second copy".to_owned()).into())
}

async fn verify_remote(
    provider: &Arc<dyn StorageProvider>,
    path: &RemotePath,
    expected: u64,
) -> Result<(), Stop> {
    let metadata = provider.stat(path).await?;
    match metadata.size_bytes {
        Some(size) if size != expected => Err(FailureReason::Other(format!(
            "the uploaded file is {size} bytes instead of {expected}, so the original was kept"
        ))
        .into()),
        _ => Ok(()),
    }
}

/// Streams a local file to the provider and returns the number of bytes sent.
async fn upload_local(
    source: &std::path::Path,
    provider: &Arc<dyn StorageProvider>,
    path: &RemotePath,
    progress: &Progress,
    cancel: &CancellationToken,
    halved: bool,
) -> Result<u64, Stop> {
    let file = tokio::fs::File::open(source).await.map_err(io_stop)?;
    let size = file.metadata().await.map_err(io_stop)?.len();
    let counter = Arc::new(AtomicU64::new(0));
    let content = {
        let counter = Arc::clone(&counter);
        let cancel = cancel.clone();
        let progress = progress.clone();
        ReaderStream::with_capacity(file, CHUNK_SIZE).map(move |chunk| {
            if cancel.is_cancelled() {
                return Err(StorageError::Cancelled);
            }
            let chunk = chunk.map_err(StorageError::Io)?;
            counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            progress.add(chunk.len(), halved);
            Ok(chunk)
        })
    };
    provider
        .write(WriteRequest {
            path: path.clone(),
            content: Box::pin(content),
            size_bytes: Some(size),
            modified_at: None,
        })
        .await?;
    Ok(counter.load(Ordering::Relaxed))
}

#[allow(clippy::too_many_arguments)]
async fn download_to_file(
    provider: &Arc<dyn StorageProvider>,
    path: &RemotePath,
    target: &std::path::Path,
    expected: u64,
    progress: &Progress,
    cancel: &CancellationToken,
    halved: bool,
    durable: bool,
) -> Result<u64, Stop> {
    let mut stream = provider
        .read(ReadRequest {
            path: path.clone(),
            range: None,
        })
        .await?;
    let mut file = tokio::fs::File::create(target).await.map_err(io_stop)?;
    let mut received = 0u64;
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        let chunk = chunk?;
        file.write_all(&chunk).await.map_err(io_stop)?;
        received += chunk.len() as u64;
        progress.add(chunk.len(), halved);
    }
    file.flush().await.map_err(io_stop)?;
    if durable {
        file.sync_all().await.map_err(io_stop)?;
    }
    if expected > 0 && received != expected {
        return Err(FailureReason::Network(format!(
            "the download ended after {received} of {expected} bytes"
        ))
        .into());
    }
    Ok(received)
}

enum LocalOutcome {
    /// Source and target are the same file; nothing was done.
    Unchanged,
    /// A move inside one volume: the file was renamed into place.
    Renamed,
    Copied,
}

/// Copies or moves one local file, in a single blocking call.
async fn transfer_local(
    source: &std::path::Path,
    target: &std::path::Path,
    moving: bool,
    progress: &Progress,
    cancel: &CancellationToken,
) -> Result<LocalOutcome, Stop> {
    let (source, target) = (source.to_path_buf(), target.to_path_buf());
    let (progress, cancel) = (progress.clone(), cancel.clone());
    tokio::task::spawn_blocking(move || {
        if same_local_file(&source, &target) {
            return Ok(LocalOutcome::Unchanged);
        }
        // A move inside one volume is a rename; anywhere else it falls back to copying.
        if moving && std::fs::rename(&source, &target).is_ok() {
            return Ok(LocalOutcome::Renamed);
        }
        copy_local_blocking(&source, &target, &progress, &cancel, moving)?;
        Ok(LocalOutcome::Copied)
    })
    .await
    .map_err(|error| Stop::Failed(FailureReason::Other(error.to_string())))?
}

fn same_local_file(first: &std::path::Path, second: &std::path::Path) -> bool {
    if first == second {
        return true;
    }
    // Resolving paths is slow, and a target that does not exist yet cannot be the source.
    if std::fs::symlink_metadata(second).is_err() {
        return false;
    }
    matches!(
        (std::fs::canonicalize(first), std::fs::canonicalize(second)),
        (Ok(first), Ok(second)) if first == second
    )
}

/// Files up to this size are copied with one call to the operating system's own copy, which is
/// much faster than moving the data through the program (no per-chunk round trips, and the OS
/// can use its fast paths). Larger files are copied in chunks so progress and cancelling work.
const NATIVE_COPY_LIMIT: u64 = 32 * 1024 * 1024;
const LOCAL_CHUNK: usize = 4 * 1024 * 1024;

/// Copies a local file through a partial file next to the target, so an interrupted copy never
/// leaves a half-written file under the real name. Fails when the source changed while it was
/// being read, because a move would otherwise delete data the copy does not have.
fn copy_local_blocking(
    source: &std::path::Path,
    target: &std::path::Path,
    progress: &Progress,
    cancel: &CancellationToken,
    durable: bool,
) -> Result<(), Stop> {
    if cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
    let size = std::fs::metadata(source).map_err(io_stop)?.len();
    // A small file that is not there yet is copied straight to its name: creating and renaming
    // a partial file doubles the folder operations, which dominate for small files. Anything
    // that replaces a file, and anything big enough to be interrupted in earnest, goes through
    // the partial file.
    let direct = size <= NATIVE_COPY_LIMIT && std::fs::symlink_metadata(target).is_err();
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let written = if direct {
        target.to_path_buf()
    } else {
        target.with_file_name(format!(".{name}.bifrost-part"))
    };
    let result = copy_into(source, size, &written, progress, cancel, durable).and_then(|()| {
        if direct {
            Ok(())
        } else {
            std::fs::rename(&written, target).map_err(io_stop)
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&written);
    }
    result
}

/// Writes `source` to `written`, checking afterwards that the source did not change meanwhile.
fn copy_into(
    source: &std::path::Path,
    size: u64,
    written: &std::path::Path,
    progress: &Progress,
    cancel: &CancellationToken,
    durable: bool,
) -> Result<(), Stop> {
    use std::io::{Read, Write};

    let copied = if size <= NATIVE_COPY_LIMIT {
        let copied = std::fs::copy(source, written).map_err(io_stop)?;
        progress.add(copied as usize, false);
        copied
    } else {
        let mut input = std::fs::File::open(source).map_err(io_stop)?;
        let mut output = std::fs::File::create(written).map_err(io_stop)?;
        let mut buffer = vec![0u8; LOCAL_CHUNK];
        let mut copied = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            let read = input.read(&mut buffer).map_err(io_stop)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read]).map_err(io_stop)?;
            copied += read as u64;
            progress.add(read, false);
        }
        // The same details the native copy keeps; neither is required for the data.
        if let Ok(metadata) = input.metadata() {
            if let Ok(modified) = metadata.modified() {
                let _ = output.set_modified(modified);
            }
            let _ = output.set_permissions(metadata.permissions());
        }
        copied
    };
    if durable {
        // Before a move removes the original, make sure the copy is on the disk.
        std::fs::OpenOptions::new()
            .write(true)
            .open(written)
            .and_then(|file| file.sync_all())
            .map_err(io_stop)?;
    }
    let now = std::fs::metadata(source).map_err(io_stop)?.len();
    if now != copied {
        return Err(FailureReason::Other(
            "the file changed while it was being copied, so the original was kept".to_owned(),
        )
        .into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_remote(
    provider: &Arc<dyn StorageProvider>,
    path: &RemotePath,
    target: &std::path::Path,
    expected: u64,
    replace: bool,
    progress: &Progress,
    cancel: &CancellationToken,
    durable: bool,
) -> Result<(), Stop> {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // As for local copies: a small file that is not there yet is written to its own name (a
    // partial file and a rename double the folder operations); anything that replaces a file,
    // or whose size is unknown or large, goes through the partial file.
    let direct = !replace
        && expected > 0
        && expected <= NATIVE_COPY_LIMIT
        && tokio::fs::symlink_metadata(target).await.is_err();
    let written = if direct {
        target.to_path_buf()
    } else {
        target.with_file_name(format!(".{name}.bifrost-part"))
    };
    let result = async {
        download_to_file(
            provider, path, &written, expected, progress, cancel, false, durable,
        )
        .await?;
        if direct {
            Ok(())
        } else {
            tokio::fs::rename(&written, target).await.map_err(io_stop)
        }
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&written).await;
    }
    result
}

#[cfg(test)]
mod tests;
