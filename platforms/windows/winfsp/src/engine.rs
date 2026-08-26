use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bifrost_common::{RemoteMetadata, RemotePath};
use bifrost_storage::{ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest};
use futures_util::StreamExt;
use tempfile::TempPath;
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex, RwLock},
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

struct StagedFile {
    path: TempPath,
    file: Mutex<File>,
}

struct ReadCache {
    offset: u64,
    data: Vec<u8>,
}

const INITIAL_READ_AHEAD_SIZE: usize = 1024 * 1024;
const SEQUENTIAL_READ_AHEAD_SIZE: usize = 16 * 1024 * 1024;

pub struct OpenFile {
    path: RwLock<RemotePath>,
    is_directory: bool,
    stage: Option<StagedFile>,
    read_cache: Mutex<Option<ReadCache>>,
    dirty: AtomicBool,
    delete_pending: AtomicBool,
}

impl OpenFile {
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

pub struct RemoteFilesystem {
    provider: Arc<dyn StorageProvider>,
    metadata_cache: RwLock<HashMap<RemotePath, Cached<RemoteMetadata>>>,
    directory_cache: RwLock<HashMap<RemotePath, Cached<Vec<RemoteEntry>>>>,
}

struct Cached<T> {
    value: T,
    cached_at: Instant,
}

const METADATA_CACHE_TTL: Duration = Duration::from_secs(5);

impl RemoteFilesystem {
    pub fn new(provider: Arc<dyn StorageProvider>) -> Self {
        Self {
            provider,
            metadata_cache: RwLock::new(HashMap::new()),
            directory_cache: RwLock::new(HashMap::new()),
        }
    }

    pub async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, WinFspFilesystemError> {
        if let Some(metadata) = self
            .metadata_cache
            .read()
            .await
            .get(path)
            .filter(|entry| entry.cached_at.elapsed() < METADATA_CACHE_TTL)
            .map(|entry| entry.value.clone())
        {
            return Ok(metadata);
        }
        let metadata = self.provider.stat(path).await?;
        self.metadata_cache.write().await.insert(
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
            .directory_cache
            .read()
            .await
            .get(path)
            .filter(|entry| entry.cached_at.elapsed() < METADATA_CACHE_TTL)
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
        let mut metadata_cache = self.metadata_cache.write().await;
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
        self.directory_cache.write().await.insert(
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
        Ok(Arc::new(OpenFile {
            path: RwLock::new(path),
            is_directory: metadata.is_directory,
            stage,
            read_cache: Mutex::new(None),
            dirty: AtomicBool::new(false),
            delete_pending: AtomicBool::new(false),
        }))
    }

    pub async fn create_file(
        &self,
        path: RemotePath,
    ) -> Result<Arc<OpenFile>, WinFspFilesystemError> {
        self.ensure_missing(&path).await?;
        Ok(Arc::new(OpenFile {
            path: RwLock::new(path),
            is_directory: false,
            stage: Some(Self::empty_stage().await?),
            read_cache: Mutex::new(None),
            dirty: AtomicBool::new(true),
            delete_pending: AtomicBool::new(false),
        }))
    }

    pub async fn create_directory(
        &self,
        path: RemotePath,
    ) -> Result<Arc<OpenFile>, WinFspFilesystemError> {
        self.ensure_missing(&path).await?;
        self.provider.create_directory(&path).await?;
        self.invalidate_caches().await;
        Ok(Arc::new(OpenFile {
            path: RwLock::new(path),
            is_directory: true,
            stage: None,
            read_cache: Mutex::new(None),
            dirty: AtomicBool::new(false),
            delete_pending: AtomicBool::new(false),
        }))
    }

    pub async fn read(
        &self,
        handle: &OpenFile,
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

        let path = handle.path().await;
        let mut cache = handle.read_cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            let relative = offset.saturating_sub(cached.offset) as usize;
            if offset >= cached.offset && relative < cached.data.len() {
                let available = (cached.data.len() - relative).min(length);
                return Ok(cached.data[relative..relative + available].to_vec());
            }
        }
        let sequential = cache
            .as_ref()
            .is_some_and(|cached| offset == cached.offset.saturating_add(cached.data.len() as u64));
        let read_length = length.max(if sequential {
            SEQUENTIAL_READ_AHEAD_SIZE
        } else {
            INITIAL_READ_AHEAD_SIZE
        });
        let end = offset.saturating_add(read_length as u64);
        let mut stream = self
            .provider
            .read(ReadRequest {
                path,
                range: Some(offset..end),
            })
            .await?;
        let mut data = Vec::with_capacity(read_length);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let remaining = read_length.saturating_sub(data.len());
            data.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if data.len() == read_length {
                break;
            }
        }
        let requested = data[..data.len().min(length)].to_vec();
        *cache = Some(ReadCache { offset, data });
        Ok(requested)
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
        handle.dirty.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn file_size(&self, handle: &OpenFile) -> Result<u64, WinFspFilesystemError> {
        if let Some(stage) = &handle.stage {
            return Ok(stage.file.lock().await.metadata().await?.len());
        }
        Ok(self
            .provider
            .stat(&handle.path().await)
            .await?
            .size_bytes
            .unwrap_or(0))
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
                path,
                content: Box::pin(content),
                size_bytes: Some(size),
                modified_at: None,
            })
            .await?;
        handle.dirty.store(false, Ordering::Release);
        self.invalidate_caches().await;
        Ok(metadata)
    }

    pub async fn delete(&self, handle: &OpenFile) -> Result<(), WinFspFilesystemError> {
        self.can_delete(handle).await?;
        let path = handle.path().await;
        self.provider.delete(&path).await?;
        self.invalidate_caches().await;
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
            Ok(_) => self.provider.delete(&destination).await?,
            Err(WinFspFilesystemError::Storage(StorageError::NotFound { .. })) => {}
            Err(error) => return Err(error),
        }
        let source = handle.path().await;
        self.provider.rename(&source, &destination).await?;
        *handle.path.write().await = destination;
        self.invalidate_caches().await;
        Ok(())
    }

    async fn ensure_missing(&self, path: &RemotePath) -> Result<(), WinFspFilesystemError> {
        match self.stat(path).await {
            Err(WinFspFilesystemError::Storage(StorageError::NotFound { .. })) => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(WinFspFilesystemError::AlreadyExists(path.clone())),
        }
    }

    async fn invalidate_caches(&self) {
        self.metadata_cache.write().await.clear();
        self.directory_cache.write().await.clear();
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
        let path = temporary.into_temp_path();
        let file = File::options().read(true).write(true).open(&path).await?;
        Ok(StagedFile {
            path,
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
    use std::time::{Duration, Instant};

    use super::*;

    static NATIVE_MOUNT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[derive(Default)]
    struct MemoryProvider {
        files: Mutex<HashMap<RemotePath, Vec<u8>>>,
        directories: Mutex<HashSet<RemotePath>>,
        capacity_delay: Duration,
        list_calls: AtomicUsize,
        stat_calls: AtomicUsize,
        read_calls: AtomicUsize,
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
                            metadata: Self::metadata(path.clone(), data.len()),
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
                .map(|data| Self::metadata(path.clone(), data.len()))
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
            Ok(Box::pin(stream::once(
                async move { Ok(Bytes::from(bytes)) },
            )))
        }

        async fn write(&self, mut request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
            let mut data = Vec::new();
            while let Some(chunk) = request.content.next().await {
                data.extend_from_slice(&chunk?);
            }
            let metadata = Self::metadata(request.path.clone(), data.len());
            self.files.lock().await.insert(request.path, data);
            Ok(metadata)
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

        filesystem.rename(&handle, destination, true).await.unwrap();
        assert_eq!(provider.contents("destination.txt").await, b"source");
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
        })
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(1));
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
}
