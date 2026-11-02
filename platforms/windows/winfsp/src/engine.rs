use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};

use bifrost_common::{RemoteMetadata, RemotePath};
use bifrost_storage::{ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest};

use crate::failure::{
    DriveFailure, DriveFailureReason, DriveOperation, FailureHandling, UploadRecovery,
};
use bytes::{Buf, BytesMut};
use futures_util::StreamExt;
use tempfile::TempPath;
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{watch, Mutex, Notify, RwLock},
    task::JoinHandle,
};
use tokio_util::io::ReaderStream;

#[derive(Debug, Error)]
pub enum WinFspFilesystemError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("invalid Windows path: {0}")]
    InvalidPath(String),
    #[error("remote item already exists: {0}")]
    AlreadyExists(RemotePath),
    #[error("remote item is a directory: {0}")]
    IsDirectory(RemotePath),
    #[error("remote item is not a directory: {0}")]
    NotDirectory(RemotePath),
    #[error("remote directory is not empty: {0}")]
    DirectoryNotEmpty(RemotePath),
    #[error("file handle is not writable: {0}")]
    NotWritable(RemotePath),
    #[error("staging operation failed: {0}")]
    Staging(String),
    #[error("local staging I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub fn remote_path_from_windows(path: &str) -> Result<RemotePath, WinFspFilesystemError> {
    let relative = path.trim_start_matches(['\\', '/']);
    RemotePath::parse(relative).map_err(|_| WinFspFilesystemError::InvalidPath(path.to_owned()))
}

/// Mirrors the remote path below the recovery folder using only Windows-safe names.
fn recovery_destination(recovery_dir: &Path, path: &RemotePath) -> PathBuf {
    let mut destination = recovery_dir.to_path_buf();
    for component in path.as_str().split('/').filter(|value| !value.is_empty()) {
        let safe = component
            .chars()
            .map(|character| match character {
                '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' => '_',
                character if character.is_control() => '_',
                character => character,
            })
            .collect::<String>();
        let safe = safe.trim_end_matches(['.', ' ']);
        destination.push(if safe.is_empty() { "_" } else { safe });
    }
    if destination == recovery_dir {
        destination.push("recovered upload");
    }
    destination
}

/// Yields `name.ext`, then `name (2).ext`, `name (3).ext`, ... so recovery never overwrites.
fn recovery_candidates(destination: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let stem = destination
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = destination
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    std::iter::once(destination.to_path_buf()).chain(
        (2..=1000)
            .map(move |index| destination.with_file_name(format!("{stem} ({index}){extension}"))),
    )
}

/// Moves staged data to the recovery folder, copying across volumes, and as a last resort
/// keeps the staged file where it is rather than deleting it.
fn persist_recovery(mut temporary: TempPath, staged: &Path, destination: &Path) -> UploadRecovery {
    let keep_in_place = |temporary: TempPath, reason: String| match temporary.keep() {
        Ok(path) => {
            tracing::warn!(%reason, "kept failed upload in the staging folder");
            UploadRecovery::Saved(path)
        }
        Err(error) => UploadRecovery::Lost(format!("{reason}; {error}")),
    };
    if let Some(parent) = destination.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return keep_in_place(temporary, error.to_string());
        }
    }
    for candidate in recovery_candidates(destination) {
        match temporary.persist_noclobber(&candidate) {
            Ok(()) => return UploadRecovery::Saved(candidate),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.path;
            }
            Err(error) => {
                let temporary = error.path;
                let copied = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&candidate)
                    .and_then(|mut target| {
                        std::io::copy(&mut std::fs::File::open(staged)?, &mut target)?;
                        target.sync_all()
                    });
                return match copied {
                    Ok(()) => UploadRecovery::Saved(candidate),
                    Err(copy_error) => {
                        let _ = std::fs::remove_file(&candidate);
                        keep_in_place(temporary, format!("{}; {copy_error}", error.error))
                    }
                };
            }
        }
    }
    keep_in_place(
        temporary,
        "no free file name in the recovery folder".to_owned(),
    )
}

struct StagedFile {
    path: PathBuf,
    /// Deletes the staged data on drop unless a failed upload hands it to recovery.
    temporary: std::sync::Mutex<Option<TempPath>>,
    file: Mutex<File>,
}

struct ReadCache {
    offset: u64,
    end: u64,
    data: BytesMut,
    generation: u64,
    complete: bool,
    error: Option<String>,
    streaming: bool,
    wanted_end: u64,
}

const INITIAL_READ_AHEAD_SIZE: usize = 1024 * 1024;
const STREAM_BUFFER_AHEAD: u64 = 32 * 1024 * 1024;
// Keeps slightly out-of-order reads inside the streaming buffer.
const STREAM_BUFFER_BEHIND: u64 = 4 * 1024 * 1024;

pub struct OpenFile {
    path: RwLock<RemotePath>,
    is_directory: bool,
    size: AtomicU64,
    stage: Option<StagedFile>,
    read_request: Mutex<()>,
    read_cache: Mutex<Option<ReadCache>>,
    read_generation: AtomicU64,
    read_progress: watch::Sender<u64>,
    read_consumed: Arc<Notify>,
    dirty: AtomicBool,
    delete_pending: AtomicBool,
    /// Set when this handle created the remote file, so a failed upload leaves nothing behind.
    created: bool,
}

impl OpenFile {
    fn new(path: RemotePath, is_directory: bool, size: u64, stage: Option<StagedFile>) -> Self {
        Self {
            path: RwLock::new(path),
            is_directory,
            size: AtomicU64::new(size),
            stage,
            read_request: Mutex::new(()),
            read_cache: Mutex::new(None),
            read_generation: AtomicU64::new(0),
            read_progress: watch::channel(0).0,
            read_consumed: Arc::new(Notify::new()),
            dirty: AtomicBool::new(false),
            delete_pending: AtomicBool::new(false),
            created: false,
        }
    }

    pub async fn path(&self) -> RemotePath {
        self.path.read().await.clone()
    }

    pub fn is_directory(&self) -> bool {
        self.is_directory
    }

    pub fn is_writable(&self) -> bool {
        self.stage.is_some()
    }

    pub fn set_delete_pending(&self, pending: bool) {
        self.delete_pending.store(pending, Ordering::Release);
    }

    pub fn delete_pending(&self) -> bool {
        self.delete_pending.load(Ordering::Acquire)
    }
}

/// Drops cached metadata for paths changed outside the drive, such as by a batch transfer
/// that wrote to the provider directly.
#[derive(Clone)]
pub struct CacheInvalidator {
    caches: Arc<MetadataCaches>,
}

impl CacheInvalidator {
    /// Each path, its descendants, and its parent listing are dropped.
    pub async fn invalidate(&self, paths: &[RemotePath]) {
        let paths = paths.iter().collect::<Vec<_>>();
        self.caches.invalidate(&paths).await;
    }
}

pub struct RemoteFilesystem {
    provider: Arc<dyn StorageProvider>,
    caches: Arc<MetadataCaches>,
    failures: FailureHandling,
}

struct Cached<T> {
    value: T,
    cached_at: Instant,
}

const METADATA_CACHE_TTL: Duration = Duration::from_secs(5);
// Used while provider change polling is healthy, matching rclone's directory cache time.
const WATCHED_METADATA_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const CHANGE_POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Waits between upload attempts after a network failure while Windows closes a file.
const UPLOAD_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];

#[derive(Default)]
struct MetadataCaches {
    metadata: RwLock<HashMap<RemotePath, Cached<RemoteMetadata>>>,
    directories: RwLock<HashMap<RemotePath, Cached<Vec<RemoteEntry>>>>,
    watched: AtomicBool,
}

impl MetadataCaches {
    fn ttl(&self) -> Duration {
        if self.watched.load(Ordering::Acquire) {
            WATCHED_METADATA_CACHE_TTL
        } else {
            METADATA_CACHE_TTL
        }
    }

    /// Drops cached state for each changed path, its descendants, and its parent listing.
    async fn invalidate(&self, paths: &[&RemotePath]) {
        self.metadata
            .write()
            .await
            .retain(|cached, _| !paths.iter().any(|path| Self::within(cached, path)));
        self.directories.write().await.retain(|cached, _| {
            !paths.iter().any(|path| {
                Self::within(cached, path)
                    || cached.as_str()
                        == path
                            .as_str()
                            .rsplit_once('/')
                            .map_or("", |(parent, _)| parent)
            })
        });
    }

    async fn clear(&self) {
        self.metadata.write().await.clear();
        self.directories.write().await.clear();
    }

    fn within(candidate: &RemotePath, path: &RemotePath) -> bool {
        let (candidate, path) = (candidate.as_str(), path.as_str());
        path.is_empty()
            || candidate == path
            || candidate
                .strip_prefix(path)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

impl RemoteFilesystem {
    pub fn new(provider: Arc<dyn StorageProvider>) -> Self {
        Self {
            provider,
            caches: Arc::new(MetadataCaches::default()),
            failures: FailureHandling::default(),
        }
    }

    /// A handle for telling the filesystem that something else changed the provider's data.
    pub fn cache_invalidator(&self) -> CacheInvalidator {
        CacheInvalidator {
            caches: Arc::clone(&self.caches),
        }
    }

    pub fn with_failure_handling(mut self, failures: FailureHandling) -> Self {
        self.failures = failures;
        self
    }

    /// Tells the host about a failed operation unless Windows alone should handle it.
    pub fn report_error(
        &self,
        operation: DriveOperation,
        path: &RemotePath,
        error: &WinFspFilesystemError,
    ) {
        if let Some(reason) = DriveFailureReason::from_error(error) {
            self.report(DriveFailure {
                operation,
                path: path.clone(),
                reason,
                recovery: None,
            });
        }
    }

    fn report(&self, failure: DriveFailure) {
        tracing::warn!(
            operation = ?failure.operation,
            path = %failure.path,
            reason = ?failure.reason,
            recovery = ?failure.recovery,
            "drive operation failed"
        );
        if let Some(sink) = &self.failures.sink {
            sink(failure);
        }
    }

    /// Polls provider change notifications and lengthens cache lifetimes while they work.
    /// Must be called inside a Tokio runtime; providers without change support end the task.
    pub fn watch_changes(&self) -> JoinHandle<()> {
        self.watch_changes_every(CHANGE_POLL_INTERVAL)
    }

    fn watch_changes_every(&self, interval: Duration) -> JoinHandle<()> {
        let provider = Arc::clone(&self.provider);
        let caches = Arc::downgrade(&self.caches);
        tokio::spawn(async move {
            let mut cursor: Option<String> = None;
            loop {
                let result = provider.poll_changes(cursor.as_deref()).await;
                let Some(caches) = caches.upgrade() else {
                    return;
                };
                match result {
                    Ok(batch) => {
                        // Entries cached before the first cursor may predate it.
                        if batch.reset || cursor.is_none() {
                            caches.clear().await;
                        } else if !batch.changed.is_empty() {
                            caches
                                .invalidate(&batch.changed.iter().collect::<Vec<_>>())
                                .await;
                        }
                        caches.watched.store(true, Ordering::Release);
                        cursor = Some(batch.cursor);
                    }
                    Err(StorageError::Unsupported { .. }) => {
                        caches.watched.store(false, Ordering::Release);
                        return;
                    }
                    Err(error) => {
                        caches.watched.store(false, Ordering::Release);
                        tracing::debug!(%error, "remote change polling failed");
                    }
                }
                drop(caches);
                tokio::time::sleep(interval).await;
            }
        })
    }

    pub async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, WinFspFilesystemError> {
        if let Some(metadata) = self
            .caches
            .metadata
            .read()
            .await
            .get(path)
            .filter(|entry| entry.cached_at.elapsed() < self.caches.ttl())
            .filter(|entry| entry.value.is_directory || entry.value.size_bytes.is_some())
            .map(|entry| entry.value.clone())
        {
            return Ok(metadata);
        }
        let metadata = self.provider.stat(path).await?;
        self.caches.metadata.write().await.insert(
            path.clone(),
            Cached {
                value: metadata.clone(),
                cached_at: Instant::now(),
            },
        );
        Ok(metadata)
    }

    pub async fn resolve_case_insensitive(
        &self,
        path: &RemotePath,
        allow_missing_leaf: bool,
    ) -> Result<RemotePath, WinFspFilesystemError> {
        if path.as_str().is_empty() {
            return Ok(RemotePath::root());
        }
        let components = path.as_str().split('/').collect::<Vec<_>>();
        let mut resolved = RemotePath::root();
        for (index, component) in components.iter().enumerate() {
            let entries = self.list(&resolved).await?;
            if let Some(entry) = entries.iter().find(|entry| {
                entry
                    .metadata
                    .path
                    .as_str()
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.to_lowercase() == component.to_lowercase())
            }) {
                resolved = entry.metadata.path.clone();
            } else if allow_missing_leaf && index + 1 == components.len() {
                return resolved
                    .join(component)
                    .map_err(|_| WinFspFilesystemError::InvalidPath(path.as_str().to_owned()));
            } else {
                return Err(StorageError::NotFound { path: path.clone() }.into());
            }
        }
        Ok(resolved)
    }

    pub async fn list(&self, path: &RemotePath) -> Result<Vec<RemoteEntry>, WinFspFilesystemError> {
        if let Some(entries) = self
            .caches
            .directories
            .read()
            .await
            .get(path)
            .filter(|entry| entry.cached_at.elapsed() < self.caches.ttl())
            .map(|entry| entry.value.clone())
        {
            return Ok(entries);
        }

        let mut entries = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.provider.list(path, cursor.as_deref()).await?;
            entries.extend(page.entries);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let cached_at = Instant::now();
        let mut metadata_cache = self.caches.metadata.write().await;
        for entry in &entries {
            metadata_cache.insert(
                entry.metadata.path.clone(),
                Cached {
                    value: entry.metadata.clone(),
                    cached_at,
                },
            );
        }
        drop(metadata_cache);
        self.caches.directories.write().await.insert(
            path.clone(),
            Cached {
                value: entries.clone(),
                cached_at,
            },
        );
        Ok(entries)
    }

    pub async fn open(
        &self,
        path: RemotePath,
        writable: bool,
    ) -> Result<Arc<OpenFile>, WinFspFilesystemError> {
        let metadata = self.stat(&path).await?;
        let stage = if writable && !metadata.is_directory {
            Some(self.stage_remote_file(&path).await?)
        } else {
            None
        };
        let size = if let Some(stage) = &stage {
            stage.file.lock().await.metadata().await?.len()
        } else {
            metadata.size_bytes.unwrap_or(0)
        };
        Ok(Arc::new(OpenFile::new(
            path,
            metadata.is_directory,
            size,
            stage,
        )))
    }

    pub async fn create_file(
        &self,
        path: RemotePath,
    ) -> Result<Arc<OpenFile>, WinFspFilesystemError> {
        self.ensure_missing(&path).await?;
        // Creating the remote file now lets Windows report a refused write before the caller
        // copies any data, instead of only at close when the error can no longer be returned.
        match self.provider.create_empty_file(&path).await {
            Ok(()) => self.invalidate_paths(&[&path]).await,
            Err(StorageError::Unsupported { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        let mut handle = OpenFile::new(path, false, 0, Some(Self::empty_stage().await?));
        handle.created = true;
        handle.dirty.store(true, Ordering::Release);
        Ok(Arc::new(handle))
    }

    pub async fn create_directory(
        &self,
        path: RemotePath,
    ) -> Result<Arc<OpenFile>, WinFspFilesystemError> {
        self.ensure_missing(&path).await?;
        self.provider.create_directory(&path).await?;
        self.invalidate_paths(&[&path]).await;
        Ok(Arc::new(OpenFile::new(path, true, 0, None)))
    }

    pub async fn read(
        &self,
        handle: &Arc<OpenFile>,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, WinFspFilesystemError> {
        if handle.is_directory {
            return Err(WinFspFilesystemError::IsDirectory(handle.path().await));
        }
        if let Some(stage) = &handle.stage {
            let mut file = stage.file.lock().await;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let mut data = vec![0; length];
            let bytes_read = file.read(&mut data).await?;
            data.truncate(bytes_read);
            return Ok(data);
        }

        let _request = handle.read_request.lock().await;
        let file_size = handle.size.load(Ordering::Acquire);
        if offset >= file_size {
            return Ok(Vec::new());
        }
        let wanted_end = offset.saturating_add(length as u64).min(file_size);
        let mut progress = handle.read_progress.subscribe();
        loop {
            let mut cache = handle.read_cache.lock().await;
            if let Some(cached) = cache.as_mut() {
                if let Some(error) = &cached.error {
                    return Err(WinFspFilesystemError::Staging(error.clone()));
                }
                if cached.streaming && offset >= cached.offset {
                    cached.wanted_end = cached.wanted_end.max(wanted_end);
                    let behind = offset - cached.offset;
                    if behind > STREAM_BUFFER_BEHIND {
                        let consumed =
                            (behind - STREAM_BUFFER_BEHIND).min(cached.data.len() as u64);
                        cached.data.advance(consumed as usize);
                        cached.offset += consumed;
                    }
                    handle.read_consumed.notify_one();
                }
                let relative = offset.saturating_sub(cached.offset) as usize;
                if offset >= cached.offset && relative < cached.data.len() {
                    let available = (cached.data.len() - relative).min(length);
                    let expected = cached.end.saturating_sub(cached.offset) as usize;
                    let remote_eof =
                        cached.complete && cached.data.len() == expected && cached.end == file_size;
                    if available == length || remote_eof {
                        return Ok(cached.data[relative..relative + available].to_vec());
                    }
                }
                let data_end = cached.offset.saturating_add(cached.data.len() as u64);
                if cached.complete && cached.end == file_size && offset >= data_end {
                    return Ok(Vec::new());
                }
                // A far forward seek would wait on a paused stream, so it starts a new range.
                let reachable =
                    !cached.streaming || offset <= data_end.saturating_add(STREAM_BUFFER_AHEAD);
                if offset >= cached.offset && offset < cached.end && !cached.complete && reachable {
                    drop(cache);
                    progress
                        .changed()
                        .await
                        .map_err(|error| WinFspFilesystemError::Staging(error.to_string()))?;
                    continue;
                }
                if offset == data_end && cached.complete && offset < cached.end {
                    return Ok(Vec::new());
                }
            }
            // Sequential access switches to one open-ended stream instead of chunked requests.
            let sequential = cache.as_ref().is_some_and(|cached| {
                cached.complete && offset == cached.offset.saturating_add(cached.data.len() as u64)
            });
            let end = if sequential {
                file_size
            } else {
                offset
                    .saturating_add(length.max(INITIAL_READ_AHEAD_SIZE) as u64)
                    .min(file_size)
            };
            let buffer_capacity = end
                .saturating_sub(offset)
                .min(STREAM_BUFFER_AHEAD + STREAM_BUFFER_BEHIND + length as u64);
            let generation = handle.read_generation.fetch_add(1, Ordering::AcqRel) + 1;
            *cache = Some(ReadCache {
                offset,
                end,
                data: BytesMut::with_capacity(buffer_capacity as usize),
                generation,
                complete: false,
                error: None,
                streaming: sequential,
                wanted_end,
            });
            drop(cache);

            tokio::spawn(Self::fetch_range(
                Arc::clone(&self.provider),
                Arc::downgrade(handle),
                Arc::clone(&handle.read_consumed),
                handle.path().await,
                generation,
                offset..end,
                sequential,
            ));
            progress
                .changed()
                .await
                .map_err(|error| WinFspFilesystemError::Staging(error.to_string()))?;
        }
    }

    async fn fetch_range(
        provider: Arc<dyn StorageProvider>,
        handle: Weak<OpenFile>,
        consumed: Arc<Notify>,
        path: RemotePath,
        generation: u64,
        range: std::ops::Range<u64>,
        streaming: bool,
    ) {
        let expected = range.end - range.start;
        let mut received = 0u64;
        let mut stream = match provider
            .read(ReadRequest {
                path,
                range: Some(range),
            })
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                Self::finish_fetch(&handle, generation, |cache| {
                    cache.error = Some(error.to_string());
                })
                .await;
                return;
            }
        };
        loop {
            let chunk = match stream.next().await {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => {
                    Self::finish_fetch(&handle, generation, |cache| {
                        // A dropped long-lived stream resumes with a new request at the gap.
                        if streaming && received > 0 {
                            cache.end = cache.offset + cache.data.len() as u64;
                        } else {
                            cache.error = Some(error.to_string());
                        }
                    })
                    .await;
                    return;
                }
                None => {
                    Self::finish_fetch(&handle, generation, |cache| {
                        if received != expected {
                            cache.error = Some(format!(
                                "remote read ended after {received} of {expected} bytes"
                            ));
                        }
                    })
                    .await;
                    return;
                }
            };
            // Backpressure: pause the stream while it is far enough ahead of the reader.
            if streaming {
                loop {
                    match Self::stream_has_room(&handle, generation).await {
                        None => return,
                        Some(true) => break,
                        Some(false) => {
                            let _ =
                                tokio::time::timeout(Duration::from_secs(1), consumed.notified())
                                    .await;
                        }
                    }
                }
            }
            let Some(handle) = handle.upgrade() else {
                return;
            };
            let full = {
                let mut cache = handle.read_cache.lock().await;
                let Some(cache) = cache
                    .as_mut()
                    .filter(|cache| cache.generation == generation)
                else {
                    return;
                };
                let take = (chunk.len() as u64).min(expected - received) as usize;
                cache.data.extend_from_slice(&chunk[..take]);
                received += take as u64;
                let full = received == expected;
                cache.complete = full;
                full
            };
            handle.read_progress.send_modify(|value| *value += 1);
            if full {
                return;
            }
        }
    }

    /// `None` means the handle closed or a newer fetch replaced this one.
    async fn stream_has_room(handle: &Weak<OpenFile>, generation: u64) -> Option<bool> {
        let handle = handle.upgrade()?;
        let cache = handle.read_cache.lock().await;
        let cache = cache
            .as_ref()
            .filter(|cache| cache.generation == generation)?;
        Some(
            cache.offset + (cache.data.len() as u64)
                < cache.wanted_end.saturating_add(STREAM_BUFFER_AHEAD),
        )
    }

    async fn finish_fetch(
        handle: &Weak<OpenFile>,
        generation: u64,
        update: impl FnOnce(&mut ReadCache),
    ) {
        let Some(handle) = handle.upgrade() else {
            return;
        };
        if let Some(cache) = handle
            .read_cache
            .lock()
            .await
            .as_mut()
            .filter(|cache| cache.generation == generation)
        {
            update(cache);
            cache.complete = true;
        }
        handle.read_progress.send_modify(|value| *value += 1);
    }

    /// Stops any background range fetch for a handle that the OS has closed.
    pub async fn close(&self, handle: &OpenFile) {
        handle.read_generation.fetch_add(1, Ordering::AcqRel);
        *handle.read_cache.lock().await = None;
        handle.read_consumed.notify_one();
    }

    pub async fn write(
        &self,
        handle: &OpenFile,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, WinFspFilesystemError> {
        let Some(stage) = &handle.stage else {
            return Err(WinFspFilesystemError::NotWritable(handle.path().await));
        };
        let mut file = stage.file.lock().await;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        file.write_all(data).await?;
        handle.dirty.store(true, Ordering::Release);
        Ok(data.len())
    }

    pub async fn append(
        &self,
        handle: &OpenFile,
        data: &[u8],
    ) -> Result<usize, WinFspFilesystemError> {
        let Some(stage) = &handle.stage else {
            return Err(WinFspFilesystemError::NotWritable(handle.path().await));
        };
        let mut file = stage.file.lock().await;
        file.seek(std::io::SeekFrom::End(0)).await?;
        file.write_all(data).await?;
        handle.dirty.store(true, Ordering::Release);
        Ok(data.len())
    }

    pub async fn set_file_size(
        &self,
        handle: &OpenFile,
        size: u64,
    ) -> Result<(), WinFspFilesystemError> {
        let Some(stage) = &handle.stage else {
            return Err(WinFspFilesystemError::NotWritable(handle.path().await));
        };
        stage.file.lock().await.set_len(size).await?;
        handle.size.store(size, Ordering::Release);
        handle.dirty.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn file_size(&self, handle: &OpenFile) -> Result<u64, WinFspFilesystemError> {
        if let Some(stage) = &handle.stage {
            return Ok(stage.file.lock().await.metadata().await?.len());
        }
        Ok(handle.size.load(Ordering::Acquire))
    }

    pub async fn flush(&self, handle: &OpenFile) -> Result<RemoteMetadata, WinFspFilesystemError> {
        let path = handle.path().await;
        if handle.is_directory {
            return self.stat(&path).await;
        }
        let Some(stage) = &handle.stage else {
            return self.stat(&path).await;
        };
        let file = stage.file.lock().await;
        if !handle.dirty.load(Ordering::Acquire) {
            return self.stat(&path).await;
        }
        file.sync_all().await?;
        let size = file.metadata().await?.len();
        let upload = File::open(&stage.path).await?;
        let content = ReaderStream::new(upload).map(|chunk| chunk.map_err(StorageError::Io));
        let metadata = self
            .provider
            .write(WriteRequest {
                path: path.clone(),
                content: Box::pin(content),
                size_bytes: Some(size),
                modified_at: None,
            })
            .await?;
        handle.dirty.store(false, Ordering::Release);
        self.invalidate_paths(&[&path]).await;
        Ok(metadata)
    }

    /// Uploads a closing handle's changes. Windows has already told the caller that the file
    /// was saved, so a failed upload keeps the staged data in the recovery folder and reports it.
    pub async fn upload_on_close(&self, handle: &OpenFile) {
        if handle.stage.is_none() || !handle.dirty.load(Ordering::Acquire) {
            return;
        }
        let mut retries = UPLOAD_RETRY_DELAYS.iter();
        let reason = loop {
            let Err(error) = self.flush(handle).await else {
                return;
            };
            let reason = DriveFailureReason::from_error(&error)
                .unwrap_or_else(|| DriveFailureReason::Local(error.to_string()));
            match retries.next() {
                Some(delay) if reason.is_transient() => tokio::time::sleep(*delay).await,
                _ => break reason,
            }
        };
        let path = handle.path().await;
        let recovery = self.preserve_stage(handle, &path).await;
        if handle.created && matches!(recovery, UploadRecovery::Saved(_)) {
            // Only this handle's own empty or partial upload can be at the path.
            match self.provider.delete(&path).await {
                Ok(()) | Err(StorageError::NotFound { .. }) => {}
                Err(error) => {
                    tracing::warn!(%error, %path, "could not remove incomplete remote upload")
                }
            }
            self.invalidate_paths(&[&path]).await;
        }
        self.report(DriveFailure {
            operation: DriveOperation::Upload,
            path,
            reason,
            recovery: Some(recovery),
        });
    }

    async fn preserve_stage(&self, handle: &OpenFile, path: &RemotePath) -> UploadRecovery {
        let Some(stage) = &handle.stage else {
            return UploadRecovery::Lost("the file had no staged data".to_owned());
        };
        let file = stage.file.lock().await;
        if let Err(error) = file.sync_all().await {
            tracing::warn!(%error, "could not flush staged data before recovery");
        }
        let Some(temporary) = stage
            .temporary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return UploadRecovery::Lost("the staged data was already released".to_owned());
        };
        let destination = recovery_destination(&self.failures.recovery_dir, path);
        let staged = stage.path.clone();
        tokio::task::spawn_blocking(move || persist_recovery(temporary, &staged, &destination))
            .await
            .unwrap_or_else(|error| UploadRecovery::Lost(error.to_string()))
    }

    pub async fn delete(&self, handle: &OpenFile) -> Result<(), WinFspFilesystemError> {
        self.can_delete(handle).await?;
        let path = handle.path().await;
        self.provider.delete(&path).await?;
        self.invalidate_paths(&[&path]).await;
        Ok(())
    }

    pub async fn can_delete(&self, handle: &OpenFile) -> Result<(), WinFspFilesystemError> {
        let path = handle.path().await;
        if handle.is_directory && !self.list(&path).await?.is_empty() {
            return Err(WinFspFilesystemError::DirectoryNotEmpty(path));
        }
        Ok(())
    }

    pub async fn rename(
        &self,
        handle: &OpenFile,
        destination: RemotePath,
        replace_if_exists: bool,
    ) -> Result<(), WinFspFilesystemError> {
        match self.stat(&destination).await {
            Ok(metadata) if !replace_if_exists || metadata.is_directory => {
                return Err(WinFspFilesystemError::AlreadyExists(destination));
            }
            Ok(_) => {
                let source = handle.path().await;
                self.provider.replace(&source, &destination).await?;
                self.invalidate_paths(&[&source, &destination]).await;
                *handle.path.write().await = destination;
                return Ok(());
            }
            Err(WinFspFilesystemError::Storage(StorageError::NotFound { .. })) => {}
            Err(error) => return Err(error),
        }
        let source = handle.path().await;
        self.provider.rename(&source, &destination).await?;
        self.invalidate_paths(&[&source, &destination]).await;
        *handle.path.write().await = destination;
        Ok(())
    }

    async fn ensure_missing(&self, path: &RemotePath) -> Result<(), WinFspFilesystemError> {
        match self.stat(path).await {
            Err(WinFspFilesystemError::Storage(StorageError::NotFound { .. })) => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(WinFspFilesystemError::AlreadyExists(path.clone())),
        }
    }

    async fn invalidate_paths(&self, paths: &[&RemotePath]) {
        self.caches.invalidate(paths).await;
    }

    async fn stage_remote_file(
        &self,
        path: &RemotePath,
    ) -> Result<StagedFile, WinFspFilesystemError> {
        let stage = Self::empty_stage().await?;
        let mut stream = self
            .provider
            .read(ReadRequest {
                path: path.clone(),
                range: None,
            })
            .await?;
        {
            let mut file = stage.file.lock().await;
            while let Some(chunk) = stream.next().await {
                file.write_all(&chunk?).await?;
            }
            file.seek(std::io::SeekFrom::Start(0)).await?;
        }
        Ok(stage)
    }

    async fn empty_stage() -> Result<StagedFile, WinFspFilesystemError> {
        let temporary = tokio::task::spawn_blocking(tempfile::NamedTempFile::new)
            .await
            .map_err(|error| WinFspFilesystemError::Staging(error.to_string()))??;
        let temporary = temporary.into_temp_path();
        let path = temporary.to_path_buf();
        let file = File::options().read(true).write(true).open(&path).await?;
        Ok(StagedFile {
            path,
            temporary: std::sync::Mutex::new(Some(temporary)),
            file: Mutex::new(file),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use bifrost_common::{CapabilitySet, ProviderKind};
    use bifrost_storage::{ByteStream, Page, StorageCapacity};
    use bytes::Bytes;
    use futures_util::stream;
    use std::time::Duration;
    #[cfg(all(target_os = "windows", feature = "native"))]
    use std::time::Instant;

    use super::*;

    #[cfg(all(target_os = "windows", feature = "native"))]
    static NATIVE_MOUNT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[derive(Default)]
    struct MemoryProvider {
        files: Mutex<HashMap<RemotePath, Vec<u8>>>,
        directories: Mutex<HashSet<RemotePath>>,
        capacity_delay: Duration,
        list_calls: AtomicUsize,
        stat_calls: AtomicUsize,
        read_calls: AtomicUsize,
        write_calls: AtomicUsize,
        replace_calls: AtomicUsize,
        advertised_size: Option<usize>,
        list_size_unknown: bool,
        read_chunk_size: usize,
        read_chunk_delay: Duration,
        supports_changes: bool,
        pending_changes: Mutex<Vec<RemotePath>>,
        supports_create_empty: bool,
        deny_create: bool,
        deny_write: bool,
        network_failures: std::sync::Mutex<usize>,
    }

    impl MemoryProvider {
        async fn insert(&self, path: &str, data: &[u8]) {
            self.files
                .lock()
                .await
                .insert(RemotePath::parse(path).unwrap(), data.to_vec());
        }

        async fn contents(&self, path: &str) -> Vec<u8> {
            self.files
                .lock()
                .await
                .get(&RemotePath::parse(path).unwrap())
                .unwrap()
                .clone()
        }

        fn metadata(path: RemotePath, size: usize) -> RemoteMetadata {
            RemoteMetadata {
                path,
                is_directory: false,
                size_bytes: Some(size as u64),
                etag: None,
                modified_at: None,
            }
        }

        fn directory_metadata(path: RemotePath) -> RemoteMetadata {
            RemoteMetadata {
                path,
                is_directory: true,
                size_bytes: None,
                etag: None,
                modified_at: None,
            }
        }

        fn is_direct_child(path: &RemotePath, parent: &RemotePath) -> bool {
            let relative = if parent.as_str().is_empty() {
                path.as_str()
            } else {
                let Some(relative) = path
                    .as_str()
                    .strip_prefix(parent.as_str())
                    .and_then(|value| value.strip_prefix('/'))
                else {
                    return false;
                };
                relative
            };
            !relative.is_empty() && !relative.contains('/')
        }
    }

    #[async_trait]
    impl StorageProvider for MemoryProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Sftp
        }

        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet::default()
        }

        async fn test_connection(&self) -> Result<(), StorageError> {
            Ok(())
        }

        async fn list(
            &self,
            prefix: &RemotePath,
            _cursor: Option<&str>,
        ) -> Result<Page<RemoteEntry>, StorageError> {
            self.list_calls.fetch_add(1, Ordering::Relaxed);
            let directories = self.directories.lock().await;
            let files = self.files.lock().await;
            let mut entries = directories
                .iter()
                .filter(|path| Self::is_direct_child(path, prefix))
                .cloned()
                .map(|path| RemoteEntry {
                    metadata: Self::directory_metadata(path),
                })
                .chain(
                    files
                        .iter()
                        .filter(|(path, _)| Self::is_direct_child(path, prefix))
                        .map(|(path, data)| RemoteEntry {
                            metadata: if self.list_size_unknown {
                                RemoteMetadata {
                                    path: path.clone(),
                                    is_directory: false,
                                    size_bytes: None,
                                    etag: None,
                                    modified_at: None,
                                }
                            } else {
                                Self::metadata(
                                    path.clone(),
                                    self.advertised_size.unwrap_or(data.len()),
                                )
                            },
                        }),
                )
                .collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.metadata.path.as_str().to_owned());
            Ok(Page {
                entries,
                next_cursor: None,
            })
        }

        async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
            self.stat_calls.fetch_add(1, Ordering::Relaxed);
            if path == &RemotePath::root() {
                return Ok(Self::directory_metadata(path.clone()));
            }
            if self.directories.lock().await.contains(path) {
                return Ok(Self::directory_metadata(path.clone()));
            }
            self.files
                .lock()
                .await
                .get(path)
                .map(|data| {
                    Self::metadata(path.clone(), self.advertised_size.unwrap_or(data.len()))
                })
                .ok_or_else(|| StorageError::NotFound { path: path.clone() })
        }

        async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
            self.read_calls.fetch_add(1, Ordering::Relaxed);
            let files = self.files.lock().await;
            let data = files
                .get(&request.path)
                .ok_or_else(|| StorageError::NotFound {
                    path: request.path.clone(),
                })?;
            let bytes = match request.range {
                Some(range) => {
                    let start = (range.start as usize).min(data.len());
                    let end = (range.end as usize).min(data.len());
                    data[start..end].to_vec()
                }
                None => data.clone(),
            };
            let chunks = if self.read_chunk_size == 0 {
                vec![Bytes::from(bytes)]
            } else {
                bytes
                    .chunks(self.read_chunk_size)
                    .map(Bytes::copy_from_slice)
                    .collect()
            };
            let delay = self.read_chunk_delay;
            Ok(Box::pin(stream::iter(chunks).then(
                move |chunk| async move {
                    tokio::time::sleep(delay).await;
                    Ok(chunk)
                },
            )))
        }

        async fn write(&self, mut request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
            self.write_calls.fetch_add(1, Ordering::Relaxed);
            if self.deny_write {
                return Err(StorageError::PermissionDenied { path: request.path });
            }
            let failing = {
                let mut left = self.network_failures.lock().unwrap();
                let failing = *left > 0;
                *left = left.saturating_sub(1);
                failing
            };
            if failing {
                return Err(StorageError::Network {
                    provider: self.kind(),
                    message: "connection reset".to_owned(),
                });
            }
            let mut data = Vec::new();
            while let Some(chunk) = request.content.next().await {
                data.extend_from_slice(&chunk?);
            }
            let metadata = Self::metadata(request.path.clone(), data.len());
            self.files.lock().await.insert(request.path, data);
            Ok(metadata)
        }

        async fn create_empty_file(&self, path: &RemotePath) -> Result<(), StorageError> {
            if !self.supports_create_empty {
                return Err(StorageError::Unsupported {
                    provider: self.kind(),
                    capability: "create_empty_file".to_owned(),
                });
            }
            if self.deny_create {
                return Err(StorageError::PermissionDenied { path: path.clone() });
            }
            self.files.lock().await.insert(path.clone(), Vec::new());
            Ok(())
        }

        async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
            self.files.lock().await.remove(path);
            self.directories.lock().await.remove(path);
            Ok(())
        }

        async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
            tokio::time::sleep(self.capacity_delay).await;
            Ok(Some(StorageCapacity {
                total_bytes: 1024,
                available_bytes: 512,
            }))
        }

        async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
            self.directories.lock().await.insert(path.clone());
            Ok(())
        }

        async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
            let mut files = self.files.lock().await;
            if let Some(data) = files.remove(from) {
                files.insert(to.clone(), data);
                return Ok(());
            }
            drop(files);
            let mut directories = self.directories.lock().await;
            if directories.remove(from) {
                directories.insert(to.clone());
                return Ok(());
            }
            Err(StorageError::NotFound { path: from.clone() })
        }

        async fn replace(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
            self.replace_calls.fetch_add(1, Ordering::Relaxed);
            self.delete(to).await?;
            self.rename(from, to).await
        }

        async fn poll_changes(
            &self,
            _cursor: Option<&str>,
        ) -> Result<bifrost_storage::ChangeBatch, StorageError> {
            if !self.supports_changes {
                return Err(StorageError::Unsupported {
                    provider: self.kind(),
                    capability: "change_notifications".to_owned(),
                });
            }
            Ok(bifrost_storage::ChangeBatch {
                changed: std::mem::take(&mut *self.pending_changes.lock().await),
                reset: false,
                cursor: "cursor".to_owned(),
            })
        }
    }

    type Reported = Arc<std::sync::Mutex<Vec<DriveFailure>>>;

    fn reporting_filesystem(
        provider: Arc<MemoryProvider>,
        recovery_dir: &Path,
    ) -> (RemoteFilesystem, Reported) {
        let reported = Reported::default();
        let sink = Arc::clone(&reported);
        let filesystem = RemoteFilesystem::new(provider).with_failure_handling(FailureHandling {
            recovery_dir: recovery_dir.to_path_buf(),
            sink: Some(Arc::new(move |failure| sink.lock().unwrap().push(failure))),
        });
        (filesystem, reported)
    }

    #[tokio::test]
    async fn create_fails_before_data_is_copied_when_the_provider_refuses_writes() {
        let provider = Arc::new(MemoryProvider {
            supports_create_empty: true,
            deny_create: true,
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, _) = reporting_filesystem(provider, recovery.path());

        let result = filesystem
            .create_file(RemotePath::parse("video.mkv").unwrap())
            .await;

        assert!(matches!(
            result,
            Err(WinFspFilesystemError::Storage(
                StorageError::PermissionDenied { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn create_makes_the_remote_file_immediately_when_supported() {
        let provider = Arc::new(MemoryProvider {
            supports_create_empty: true,
            ..Default::default()
        });
        let filesystem = RemoteFilesystem::new(provider.clone());

        filesystem
            .create_file(RemotePath::parse("notes.txt").unwrap())
            .await
            .unwrap();

        assert_eq!(provider.contents("notes.txt").await, b"");
    }

    #[tokio::test]
    async fn failed_close_upload_keeps_the_data_and_reports_where() {
        let provider = Arc::new(MemoryProvider {
            deny_write: true,
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, reported) = reporting_filesystem(provider.clone(), recovery.path());
        let path = RemotePath::parse("Filmer/2026-10-31_16-10-30.mkv").unwrap();
        let handle = filesystem.create_file(path.clone()).await.unwrap();
        filesystem.write(&handle, 0, b"recording").await.unwrap();
        let staged = handle.stage.as_ref().unwrap().path.clone();

        filesystem.upload_on_close(&handle).await;
        drop(handle);

        let expected = recovery
            .path()
            .join("Filmer")
            .join("2026-10-31_16-10-30.mkv");
        assert_eq!(std::fs::read(&expected).unwrap(), b"recording");
        assert!(!staged.exists());
        assert_eq!(
            *reported.lock().unwrap(),
            vec![DriveFailure {
                operation: DriveOperation::Upload,
                path,
                reason: DriveFailureReason::PermissionDenied,
                recovery: Some(UploadRecovery::Saved(expected)),
            }]
        );
    }

    #[tokio::test]
    async fn failed_upload_of_a_new_file_removes_its_empty_remote_placeholder() {
        let provider = Arc::new(MemoryProvider {
            supports_create_empty: true,
            deny_write: true,
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, reported) = reporting_filesystem(provider.clone(), recovery.path());
        let handle = filesystem
            .create_file(RemotePath::parse("video.mkv").unwrap())
            .await
            .unwrap();
        filesystem.write(&handle, 0, b"recording").await.unwrap();

        filesystem.upload_on_close(&handle).await;

        assert!(provider.files.lock().await.is_empty());
        assert_eq!(reported.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn failed_upload_of_an_existing_file_leaves_the_remote_copy_alone() {
        let provider = Arc::new(MemoryProvider {
            deny_write: true,
            ..Default::default()
        });
        provider.insert("notes.txt", b"before").await;
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, _) = reporting_filesystem(provider.clone(), recovery.path());
        let handle = filesystem
            .open(RemotePath::parse("notes.txt").unwrap(), true)
            .await
            .unwrap();
        filesystem.write(&handle, 0, b"after!").await.unwrap();

        filesystem.upload_on_close(&handle).await;

        assert_eq!(provider.contents("notes.txt").await, b"before");
        assert_eq!(
            std::fs::read(recovery.path().join("notes.txt")).unwrap(),
            b"after!"
        );
    }

    #[tokio::test]
    async fn recovery_never_overwrites_an_earlier_recovered_file() {
        let provider = Arc::new(MemoryProvider {
            deny_write: true,
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        std::fs::write(recovery.path().join("video.mkv"), b"earlier").unwrap();
        let (filesystem, _) = reporting_filesystem(provider, recovery.path());
        let handle = filesystem
            .create_file(RemotePath::parse("video.mkv").unwrap())
            .await
            .unwrap();
        filesystem.write(&handle, 0, b"later").await.unwrap();

        filesystem.upload_on_close(&handle).await;

        assert_eq!(
            std::fs::read(recovery.path().join("video.mkv")).unwrap(),
            b"earlier"
        );
        assert_eq!(
            std::fs::read(recovery.path().join("video (2).mkv")).unwrap(),
            b"later"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn close_upload_retries_network_failures_before_giving_up() {
        let provider = Arc::new(MemoryProvider {
            network_failures: std::sync::Mutex::new(2),
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, reported) = reporting_filesystem(provider.clone(), recovery.path());
        let handle = filesystem
            .create_file(RemotePath::parse("video.mkv").unwrap())
            .await
            .unwrap();
        filesystem.write(&handle, 0, b"recording").await.unwrap();

        filesystem.upload_on_close(&handle).await;

        assert_eq!(provider.contents("video.mkv").await, b"recording");
        assert!(reported.lock().unwrap().is_empty());
        assert_eq!(provider.write_calls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn unchanged_writable_handles_do_not_upload_or_report_on_close() {
        let provider = Arc::new(MemoryProvider {
            deny_write: true,
            ..Default::default()
        });
        provider.insert("notes.txt", b"before").await;
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, reported) = reporting_filesystem(provider.clone(), recovery.path());
        let handle = filesystem
            .open(RemotePath::parse("notes.txt").unwrap(), true)
            .await
            .unwrap();

        filesystem.upload_on_close(&handle).await;

        assert_eq!(provider.write_calls.load(Ordering::Relaxed), 0);
        assert!(reported.lock().unwrap().is_empty());
    }

    #[test]
    fn recovery_paths_mirror_the_remote_path_with_safe_names() {
        let root = Path::new(r"C:\Recovered");
        assert_eq!(
            recovery_destination(root, &RemotePath::parse("Filmer/clip|1?.mkv.").unwrap()),
            root.join("Filmer").join("clip_1_.mkv")
        );
    }

    #[tokio::test]
    async fn invalidator_makes_files_written_outside_the_drive_visible() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("dest/old.txt", b"old").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let invalidator = filesystem.cache_invalidator();
        let folder = RemotePath::parse("dest").unwrap();
        assert_eq!(filesystem.list(&folder).await.unwrap().len(), 1);

        provider.insert("dest/new.txt", b"new").await;
        assert_eq!(filesystem.list(&folder).await.unwrap().len(), 1);
        invalidator.invalidate(std::slice::from_ref(&folder)).await;

        assert_eq!(filesystem.list(&folder).await.unwrap().len(), 2);
    }

    #[test]
    fn normalizes_windows_paths() {
        assert_eq!(
            remote_path_from_windows(r"\docs\report.txt")
                .unwrap()
                .as_str(),
            "docs/report.txt"
        );
        assert_eq!(remote_path_from_windows(r"\").unwrap(), RemotePath::root());
        assert!(remote_path_from_windows(r"\docs\..\secret.txt").is_err());
    }

    #[tokio::test]
    async fn directory_listing_warms_child_metadata_cache() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("video.mp4", b"content").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let root = RemotePath::root();
        let file = RemotePath::parse("video.mp4").unwrap();

        filesystem.list(&root).await.unwrap();
        filesystem.stat(&file).await.unwrap();
        filesystem
            .resolve_case_insensitive(&file, false)
            .await
            .unwrap();

        assert_eq!(provider.list_calls.load(Ordering::Relaxed), 1);
        assert_eq!(provider.stat_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn open_refreshes_listed_file_when_size_is_unknown() {
        let provider = Arc::new(MemoryProvider {
            list_size_unknown: true,
            ..Default::default()
        });
        provider.insert("document.docx", b"exported document").await;
        let filesystem = RemoteFilesystem::new(provider.clone());

        filesystem.list(&RemotePath::root()).await.unwrap();
        let handle = filesystem
            .open(RemotePath::parse("document.docx").unwrap(), false)
            .await
            .unwrap();

        assert_eq!(handle.size.load(Ordering::Acquire), 17);
        assert_eq!(provider.stat_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn adjacent_reads_share_one_provider_range_request() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("video.mp4", b"thumbnail payload").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let handle = filesystem
            .open(RemotePath::parse("video.mp4").unwrap(), false)
            .await
            .unwrap();

        assert_eq!(filesystem.read(&handle, 0, 4).await.unwrap(), b"thum");
        assert_eq!(filesystem.read(&handle, 4, 5).await.unwrap(), b"bnail");

        assert_eq!(provider.read_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn read_returns_when_the_requested_bytes_arrive() {
        let provider = Arc::new(MemoryProvider {
            read_chunk_size: 4,
            read_chunk_delay: Duration::from_millis(200),
            ..Default::default()
        });
        provider.insert("video.mp4", b"abcdefgh").await;
        let filesystem = RemoteFilesystem::new(provider);
        let handle = filesystem
            .open(RemotePath::parse("video.mp4").unwrap(), false)
            .await
            .unwrap();

        let first =
            tokio::time::timeout(Duration::from_millis(300), filesystem.read(&handle, 0, 4))
                .await
                .expect("read waited for the complete prefetch range")
                .unwrap();

        assert_eq!(first, b"abcd");
    }

    #[tokio::test]
    async fn read_does_not_report_an_intermediate_chunk_as_eof() {
        let provider = Arc::new(MemoryProvider {
            read_chunk_size: 4,
            read_chunk_delay: Duration::from_millis(100),
            ..Default::default()
        });
        provider.insert("video.mp4", b"abcdefgh").await;
        let filesystem = RemoteFilesystem::new(provider);
        let handle = filesystem
            .open(RemotePath::parse("video.mp4").unwrap(), false)
            .await
            .unwrap();

        let data = filesystem.read(&handle, 0, 8).await.unwrap();

        assert_eq!(data, b"abcdefgh");
    }

    #[tokio::test]
    async fn concurrent_reads_share_the_active_prefetch() {
        let provider = Arc::new(MemoryProvider {
            read_chunk_size: 4,
            read_chunk_delay: Duration::from_millis(50),
            ..Default::default()
        });
        provider.insert("video.mp4", b"abcdefghijkl").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let handle = filesystem
            .open(RemotePath::parse("video.mp4").unwrap(), false)
            .await
            .unwrap();

        let (first, second) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(
                filesystem.read(&handle, 0, 4),
                filesystem.read(&handle, 4, 4),
            )
        })
        .await
        .expect("concurrent reads starved each other's prefetch");

        assert_eq!(first.unwrap(), b"abcd");
        assert_eq!(second.unwrap(), b"efgh");
        assert_eq!(provider.read_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn read_returns_a_short_buffer_only_at_remote_eof() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("video.mp4", b"abcdef").await;
        let filesystem = RemoteFilesystem::new(provider);
        let handle = filesystem
            .open(RemotePath::parse("video.mp4").unwrap(), false)
            .await
            .unwrap();

        assert_eq!(filesystem.read(&handle, 0, 8).await.unwrap(), b"abcdef");
        assert!(filesystem.read(&handle, 6, 8).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn read_rejects_a_stream_that_ends_before_advertised_eof() {
        let provider = Arc::new(MemoryProvider {
            advertised_size: Some(8),
            ..Default::default()
        });
        provider.insert("video.mp4", b"abcd").await;
        let filesystem = RemoteFilesystem::new(provider);
        let handle = filesystem
            .open(RemotePath::parse("video.mp4").unwrap(), false)
            .await
            .unwrap();

        let error = filesystem.read(&handle, 0, 8).await.unwrap_err();

        assert!(matches!(error, WinFspFilesystemError::Staging(_)));
        assert!(error.to_string().contains("4 of 8 bytes"));
    }

    #[tokio::test]
    async fn partial_write_is_staged_and_preserves_other_bytes_on_flush() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("notes.txt", b"hello world").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let handle = filesystem
            .open(RemotePath::parse("notes.txt").unwrap(), true)
            .await
            .unwrap();

        filesystem.write(&handle, 6, b"Rust!").await.unwrap();
        assert_eq!(provider.contents("notes.txt").await, b"hello world");
        assert_eq!(
            filesystem.read(&handle, 0, 11).await.unwrap(),
            b"hello Rust!"
        );

        filesystem.flush(&handle).await.unwrap();
        assert_eq!(provider.contents("notes.txt").await, b"hello Rust!");
    }

    #[tokio::test]
    async fn opening_without_changes_does_not_upload() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("document.docx", b"original export").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let handle = filesystem
            .open(RemotePath::parse("document.docx").unwrap(), true)
            .await
            .unwrap();

        filesystem.flush(&handle).await.unwrap();

        assert_eq!(provider.write_calls.load(Ordering::Relaxed), 0);
        assert_eq!(provider.contents("document.docx").await, b"original export");
    }

    #[tokio::test]
    async fn writable_open_uses_the_complete_staged_file_size() {
        let provider = Arc::new(MemoryProvider {
            advertised_size: Some(4),
            ..Default::default()
        });
        provider.insert("document.docx", b"complete document").await;
        let filesystem = RemoteFilesystem::new(provider);

        let handle = filesystem
            .open(RemotePath::parse("document.docx").unwrap(), true)
            .await
            .unwrap();

        assert_eq!(handle.size.load(Ordering::Acquire), 17);
        assert_eq!(
            filesystem.read(&handle, 0, 32).await.unwrap(),
            b"complete document"
        );
    }

    #[tokio::test]
    async fn create_refuses_to_overwrite_existing_file() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("existing.txt", b"keep").await;
        let filesystem = RemoteFilesystem::new(provider);

        let result = filesystem
            .create_file(RemotePath::parse("existing.txt").unwrap())
            .await;
        assert!(matches!(
            result,
            Err(WinFspFilesystemError::AlreadyExists(_))
        ));
    }

    #[tokio::test]
    async fn remote_changes_extend_cache_lifetime_and_invalidate_changed_paths() {
        let provider = Arc::new(MemoryProvider {
            supports_changes: true,
            ..Default::default()
        });
        provider.insert("a/one.txt", b"one").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let folder = RemotePath::parse("a").unwrap();
        filesystem.list(&folder).await.unwrap();

        let watcher = filesystem.watch_changes_every(Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(filesystem.caches.ttl(), WATCHED_METADATA_CACHE_TTL);
        filesystem.list(&folder).await.unwrap();
        filesystem.list(&folder).await.unwrap();
        assert_eq!(provider.list_calls.load(Ordering::Relaxed), 2);

        provider
            .pending_changes
            .lock()
            .await
            .push(RemotePath::parse("a/two.txt").unwrap());
        tokio::time::sleep(Duration::from_millis(50)).await;
        filesystem.list(&folder).await.unwrap();
        assert_eq!(provider.list_calls.load(Ordering::Relaxed), 3);
        watcher.abort();
    }

    #[tokio::test]
    async fn providers_without_change_support_keep_the_short_cache_lifetime() {
        let provider = Arc::new(MemoryProvider::default());
        let filesystem = RemoteFilesystem::new(provider);

        filesystem.watch_changes().await.unwrap();

        assert_eq!(filesystem.caches.ttl(), METADATA_CACHE_TTL);
    }

    #[tokio::test]
    async fn sequential_reads_share_one_open_ended_stream() {
        let provider = Arc::new(MemoryProvider {
            read_chunk_size: 1024 * 1024,
            ..Default::default()
        });
        let expected = (0..40 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        provider.insert("movie.mkv", &expected).await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let handle = filesystem
            .open(RemotePath::parse("movie.mkv").unwrap(), false)
            .await
            .unwrap();

        let mut actual = Vec::new();
        loop {
            let chunk = filesystem
                .read(&handle, actual.len() as u64, 1024 * 1024)
                .await
                .unwrap();
            if chunk.is_empty() {
                break;
            }
            actual.extend_from_slice(&chunk);
        }

        assert!(actual == expected);
        assert_eq!(provider.read_calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn deleting_a_file_keeps_unrelated_directory_listings_cached() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("a/one.txt", b"one").await;
        provider.insert("b/two.txt", b"two").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let first = RemotePath::parse("a").unwrap();
        let second = RemotePath::parse("b").unwrap();
        filesystem.list(&first).await.unwrap();
        filesystem.list(&second).await.unwrap();

        let handle = filesystem
            .open(RemotePath::parse("a/one.txt").unwrap(), false)
            .await
            .unwrap();
        filesystem.delete(&handle).await.unwrap();
        filesystem.list(&second).await.unwrap();
        assert_eq!(provider.list_calls.load(Ordering::Relaxed), 2);

        assert!(filesystem.list(&first).await.unwrap().is_empty());
        assert_eq!(provider.list_calls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn rename_requires_explicit_replace() {
        let provider = Arc::new(MemoryProvider::default());
        provider.insert("source.txt", b"source").await;
        provider.insert("destination.txt", b"destination").await;
        let filesystem = RemoteFilesystem::new(provider.clone());
        let handle = filesystem
            .open(RemotePath::parse("source.txt").unwrap(), false)
            .await
            .unwrap();
        let destination = RemotePath::parse("destination.txt").unwrap();

        let collision = filesystem.rename(&handle, destination.clone(), false).await;
        assert!(matches!(
            collision,
            Err(WinFspFilesystemError::AlreadyExists(_))
        ));
        assert_eq!(provider.replace_calls.load(Ordering::Relaxed), 0);

        filesystem.rename(&handle, destination, true).await.unwrap();
        assert_eq!(provider.contents("destination.txt").await, b"source");
        assert_eq!(provider.replace_calls.load(Ordering::Relaxed), 1);
    }

    #[cfg(all(target_os = "windows", feature = "native"))]
    #[test]
    fn native_mount_does_not_wait_for_capacity() {
        let _guard = NATIVE_MOUNT_TEST_LOCK.lock().unwrap();
        let drive_letter = ('M'..='Z')
            .rev()
            .find(|letter| !std::path::Path::new(&format!(r"{letter}:\")).exists())
            .expect("no free drive letter available for WinFsp acceptance test");
        let provider = Arc::new(MemoryProvider {
            capacity_delay: Duration::from_secs(5),
            ..Default::default()
        });

        let started = Instant::now();
        let mount = crate::mount(crate::MountConfig {
            drive_letter,
            volume_label: "Bifrost Capacity Test".to_owned(),
            network_drive: true,
            icon_source: None,
            provider,
            failures: Default::default(),
        })
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(1));
        mount.unmount();
    }

    #[cfg(all(target_os = "windows", feature = "native"))]
    #[test]
    fn native_mount_reads_a_multi_chunk_file_without_short_reads() {
        let _guard = NATIVE_MOUNT_TEST_LOCK.lock().unwrap();
        let drive_letter = ('M'..='Z')
            .rev()
            .find(|letter| !std::path::Path::new(&format!(r"{letter}:\")).exists())
            .expect("no free drive letter available for WinFsp acceptance test");
        let expected = (0..4 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let provider = Arc::new(MemoryProvider {
            read_chunk_size: 64 * 1024,
            ..Default::default()
        });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(provider.insert("stream.bin", &expected));

        let mount = crate::mount(crate::MountConfig {
            drive_letter,
            volume_label: "Bifrost Stream Test".to_owned(),
            network_drive: false,
            icon_source: None,
            provider,
            failures: Default::default(),
        })
        .unwrap();
        let actual = std::fs::read(format!(r"{drive_letter}:\stream.bin")).unwrap();

        assert_eq!(actual, expected);
        mount.unmount();
    }

    #[cfg(all(target_os = "windows", feature = "native"))]
    #[test]
    fn native_mount_reads_and_writes_through_a_drive_letter() {
        let _guard = NATIVE_MOUNT_TEST_LOCK.lock().unwrap();
        let drive_letter = ('M'..='Z')
            .rev()
            .find(|letter| !std::path::Path::new(&format!(r"{letter}:\")).exists())
            .expect("no free drive letter available for WinFsp acceptance test");
        let provider = Arc::new(MemoryProvider::default());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(provider.insert("notes.txt", b"before"));

        let mount = crate::mount(crate::MountConfig {
            drive_letter,
            volume_label: "Bifrost Test".to_owned(),
            network_drive: false,
            icon_source: None,
            provider: provider.clone(),
            failures: Default::default(),
        })
        .unwrap();
        let path = format!(r"{drive_letter}:\notes.txt");

        assert_eq!(std::fs::read(&path).unwrap(), b"before");
        std::fs::write(&path, b"after").unwrap();
        assert_eq!(runtime.block_on(provider.contents("notes.txt")), b"after");

        let directory = format!(r"{drive_letter}:\projects");
        let created = format!(r"{directory}\draft.txt");
        let renamed = format!(r"{directory}\final.txt");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(&created, b"document").unwrap();
        std::fs::rename(&created, &renamed).unwrap();
        assert_eq!(
            runtime.block_on(provider.contents("projects/final.txt")),
            b"document"
        );
        assert_eq!(std::fs::read(&renamed).unwrap(), b"document");
        std::fs::remove_file(&renamed).unwrap();
        std::fs::remove_dir(&directory).unwrap();

        mount.unmount();
        assert!(!std::path::Path::new(&format!(r"{drive_letter}:\")).exists());
    }

    #[cfg(all(target_os = "windows", feature = "native"))]
    fn native_test_mount(
        provider: Arc<MemoryProvider>,
        failures: FailureHandling,
    ) -> (crate::MountHandle, char) {
        let drive_letter = ('M'..='Z')
            .rev()
            .find(|letter| !std::path::Path::new(&format!(r"{letter}:\")).exists())
            .expect("no free drive letter available for WinFsp acceptance test");
        let mount = crate::mount(crate::MountConfig {
            drive_letter,
            volume_label: "Bifrost Failure Test".to_owned(),
            network_drive: false,
            icon_source: None,
            provider,
            failures,
        })
        .unwrap();
        (mount, drive_letter)
    }

    #[cfg(all(target_os = "windows", feature = "native"))]
    #[test]
    fn native_copy_into_a_refused_folder_fails_instead_of_reporting_success() {
        let _guard = NATIVE_MOUNT_TEST_LOCK.lock().unwrap();
        let provider = Arc::new(MemoryProvider {
            supports_create_empty: true,
            deny_create: true,
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, reported) = reporting_filesystem(provider.clone(), recovery.path());
        let (mount, drive_letter) = native_test_mount(provider, filesystem.failures.clone());

        let error = std::fs::write(format!(r"{drive_letter}:\video.mkv"), b"recording")
            .expect_err("a refused create must reach the caller");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            reported.lock().unwrap()[0].operation,
            DriveOperation::CreateFile
        );
        mount.unmount();
    }

    #[cfg(all(target_os = "windows", feature = "native"))]
    #[test]
    fn native_failed_upload_on_close_keeps_the_data_in_the_recovery_folder() {
        let _guard = NATIVE_MOUNT_TEST_LOCK.lock().unwrap();
        let provider = Arc::new(MemoryProvider {
            deny_write: true,
            ..Default::default()
        });
        let recovery = tempfile::tempdir().unwrap();
        let (filesystem, reported) = reporting_filesystem(provider.clone(), recovery.path());
        let (mount, drive_letter) = native_test_mount(provider, filesystem.failures.clone());

        // Windows cannot fail a close, so the write itself still appears to succeed.
        std::fs::write(format!(r"{drive_letter}:\video.mkv"), b"recording").unwrap();

        assert_eq!(
            std::fs::read(recovery.path().join("video.mkv")).unwrap(),
            b"recording"
        );
        let reported = reported.lock().unwrap();
        assert_eq!(reported[0].operation, DriveOperation::Upload);
        assert_eq!(reported[0].reason, DriveFailureReason::PermissionDenied);
        drop(reported);
        mount.unmount();
    }
}
