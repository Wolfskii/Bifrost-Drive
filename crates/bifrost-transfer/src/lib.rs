use bifrost_cache::{CacheError, CacheManager, CacheRecord};
use bifrost_common::{ConnectionId, RemotePath, TransferId};
use bifrost_storage::{StorageError, StorageProvider};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Download,
    Upload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TransferJob {
    pub id: TransferId,
    pub connection_id: ConnectionId,
    pub path: RemotePath,
    pub direction: TransferDirection,
    pub total_bytes: Option<u64>,
    pub transferred_bytes: u64,
    pub attempts: u32,
    pub status: TransferStatus,
    pub next_retry_at: Option<SystemTime>,
    sequence: u64,
}

#[derive(Debug, Clone)]
pub struct TransferSnapshot {
    pub id: TransferId,
    pub connection_id: ConnectionId,
    pub path: RemotePath,
    pub direction: TransferDirection,
    pub total_bytes: Option<u64>,
    pub transferred_bytes: u64,
    pub attempts: u32,
    pub status: TransferStatus,
    pub next_retry_at: Option<SystemTime>,
}

#[async_trait::async_trait]
pub trait TransferStore: Send + Sync {
    async fn save(&self, transfer: TransferSnapshot) -> Result<(), String>;
    async fn load_recoverable(&self) -> Result<Vec<TransferSnapshot>, String>;

    async fn save_cache(&self, _record: CacheRecord) -> Result<(), String> {
        Ok(())
    }

    async fn load_cache(&self) -> Result<Vec<CacheRecord>, String> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("transfer {0} was not found")]
    NotFound(String),
    #[error("transfer {0} cannot be changed while {1:?}")]
    InvalidTransition(String, TransferStatus),
    #[error("transfer queue has reached its concurrency limit")]
    AtCapacity,
}

#[derive(Debug)]
pub struct TransferQueue {
    max_concurrent: usize,
    max_attempts: u32,
    active: usize,
    next_sequence: u64,
    jobs: HashMap<TransferId, TransferJob>,
}

impl TransferQueue {
    pub fn new(max_concurrent: usize, max_attempts: u32) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            max_attempts: max_attempts.max(1),
            active: 0,
            next_sequence: 0,
            jobs: HashMap::new(),
        }
    }

    pub fn enqueue(
        &mut self,
        connection_id: ConnectionId,
        path: RemotePath,
        direction: TransferDirection,
        total_bytes: Option<u64>,
    ) -> TransferId {
        let id = TransferId::new();
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.jobs.insert(
            id,
            TransferJob {
                id,
                connection_id,
                path,
                direction,
                total_bytes,
                transferred_bytes: 0,
                attempts: 0,
                status: TransferStatus::Pending,
                next_retry_at: None,
                sequence,
            },
        );
        id
    }

    pub fn start_available(&mut self, now: SystemTime) -> Vec<TransferId> {
        let capacity = self.max_concurrent.saturating_sub(self.active);
        let mut pending: Vec<_> = self
            .jobs
            .values_mut()
            .filter(|job| {
                job.status == TransferStatus::Pending
                    && job.next_retry_at.is_none_or(|retry_at| retry_at <= now)
            })
            .collect();
        pending.sort_by_key(|job| job.sequence);
        pending.truncate(capacity);

        let started: Vec<_> = pending
            .into_iter()
            .map(|job| {
                job.status = TransferStatus::Running;
                job.attempts += 1;
                job.next_retry_at = None;
                job.id
            })
            .collect();
        self.active += started.len();
        started
    }

    pub fn start(&mut self, id: TransferId, now: SystemTime) -> Result<(), TransferError> {
        if self.active >= self.max_concurrent {
            return Err(TransferError::AtCapacity);
        }
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Pending
            || job.next_retry_at.is_some_and(|retry_at| retry_at > now)
        {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.status = TransferStatus::Running;
        job.attempts += 1;
        job.next_retry_at = None;
        self.active += 1;
        Ok(())
    }

    pub fn update_progress(
        &mut self,
        id: TransferId,
        transferred_bytes: u64,
    ) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Running {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.transferred_bytes = match job.total_bytes {
            Some(total) => transferred_bytes.min(total),
            None => transferred_bytes,
        };
        Ok(())
    }

    pub fn complete(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Running {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.status = TransferStatus::Completed;
        self.active = self.active.saturating_sub(1);
        Ok(())
    }

    pub fn fail(
        &mut self,
        id: TransferId,
        retryable: bool,
        now: SystemTime,
    ) -> Result<(), TransferError> {
        let (status, attempts) = {
            let job = self.get(id)?;
            (job.status, job.attempts)
        };
        if status != TransferStatus::Running {
            return Err(TransferError::InvalidTransition(id.to_string(), status));
        }
        self.active = self.active.saturating_sub(1);
        let max_attempts = self.max_attempts;
        let job = self.job_mut(id)?;
        if retryable && attempts < max_attempts {
            let exponent = attempts.min(7);
            job.status = TransferStatus::Pending;
            job.next_retry_at = Some(now + Duration::from_secs(2u64.pow(exponent)));
        } else {
            job.status = TransferStatus::Failed;
        }
        Ok(())
    }

    pub fn pause(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        match job.status {
            TransferStatus::Pending => job.status = TransferStatus::Paused,
            TransferStatus::Running => {
                job.status = TransferStatus::Paused;
                self.active = self.active.saturating_sub(1);
            }
            status => return Err(TransferError::InvalidTransition(id.to_string(), status)),
        }
        Ok(())
    }

    pub fn resume(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Paused {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.status = TransferStatus::Pending;
        Ok(())
    }

    pub fn cancel(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        match job.status {
            TransferStatus::Pending | TransferStatus::Paused => {
                job.status = TransferStatus::Cancelled
            }
            TransferStatus::Running => {
                job.status = TransferStatus::Cancelled;
                self.active = self.active.saturating_sub(1);
            }
            status => return Err(TransferError::InvalidTransition(id.to_string(), status)),
        }
        Ok(())
    }

    pub fn get(&self, id: TransferId) -> Result<&TransferJob, TransferError> {
        self.jobs
            .get(&id)
            .ok_or_else(|| TransferError::NotFound(id.to_string()))
    }

    fn restore(&mut self, mut snapshot: TransferSnapshot) {
        if snapshot.status == TransferStatus::Running {
            snapshot.status = TransferStatus::Pending;
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.jobs.insert(
            snapshot.id,
            TransferJob {
                id: snapshot.id,
                connection_id: snapshot.connection_id,
                path: snapshot.path,
                direction: snapshot.direction,
                total_bytes: snapshot.total_bytes,
                transferred_bytes: snapshot.transferred_bytes,
                attempts: snapshot.attempts,
                status: snapshot.status,
                next_retry_at: snapshot.next_retry_at,
                sequence,
            },
        );
    }

    fn snapshot(&self, id: TransferId) -> Result<TransferSnapshot, TransferError> {
        let job = self.get(id)?;
        Ok(TransferSnapshot {
            id: job.id,
            connection_id: job.connection_id,
            path: job.path.clone(),
            direction: job.direction,
            total_bytes: job.total_bytes,
            transferred_bytes: job.transferred_bytes,
            attempts: job.attempts,
            status: job.status,
            next_retry_at: job.next_retry_at,
        })
    }

    fn job_mut(&mut self, id: TransferId) -> Result<&mut TransferJob, TransferError> {
        self.jobs
            .get_mut(&id)
            .ok_or_else(|| TransferError::NotFound(id.to_string()))
    }
}

#[derive(Debug, Error)]
pub enum TransferServiceError {
    #[error(transparent)]
    Queue(#[from] TransferError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("cache commit task failed: {0}")]
    CommitTask(String),
    #[error("transfer persistence failed: {0}")]
    Persistence(String),
}

pub struct TransferService {
    queue: Mutex<TransferQueue>,
    cache: Arc<Mutex<CacheManager>>,
    store: Option<Arc<dyn TransferStore>>,
}

impl TransferService {
    pub fn new(cache: CacheManager, max_concurrent: usize, max_attempts: u32) -> Self {
        Self::with_store(cache, max_concurrent, max_attempts, None)
    }

    pub fn with_store(
        cache: CacheManager,
        max_concurrent: usize,
        max_attempts: u32,
        store: Option<Arc<dyn TransferStore>>,
    ) -> Self {
        Self {
            queue: Mutex::new(TransferQueue::new(max_concurrent, max_attempts)),
            cache: Arc::new(Mutex::new(cache)),
            store,
        }
    }

    pub async fn recover(&self) -> Result<usize, TransferServiceError> {
        let Some(store) = &self.store else {
            return Ok(0);
        };
        let cache_records = store
            .load_cache()
            .await
            .map_err(TransferServiceError::Persistence)?;
        {
            let mut cache = self.cache.lock().expect("cache manager poisoned");
            for record in cache_records {
                cache.restore(record)?;
            }
        }
        let snapshots = store
            .load_recoverable()
            .await
            .map_err(TransferServiceError::Persistence)?;
        let count = snapshots.len();
        let mut queue = self.queue.lock().expect("transfer queue poisoned");
        for snapshot in snapshots {
            queue.restore(snapshot);
        }
        Ok(count)
    }

    async fn persist(&self, id: TransferId) -> Result<(), TransferServiceError> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let snapshot = self
            .queue
            .lock()
            .expect("transfer queue poisoned")
            .snapshot(id)?;
        store
            .save(snapshot)
            .await
            .map_err(TransferServiceError::Persistence)
    }

    pub async fn hydrate(
        &self,
        provider: &dyn StorageProvider,
        connection_id: ConnectionId,
        path: RemotePath,
        total_bytes: Option<u64>,
        pinned: bool,
    ) -> Result<PathBuf, TransferServiceError> {
        let transfer_id = {
            let mut queue = self.queue.lock().expect("transfer queue poisoned");
            let id = queue.enqueue(
                connection_id,
                path.clone(),
                TransferDirection::Download,
                total_bytes,
            );
            queue.start(id, SystemTime::now())?;
            id
        };
        self.persist(transfer_id).await?;
        let temporary_path = self
            .cache
            .lock()
            .expect("cache manager poisoned")
            .temporary_path(transfer_id);
        let result = self
            .download_to_temp(provider, &path, &temporary_path, transfer_id)
            .await;
        let size_bytes = match result {
            Ok(size_bytes) => size_bytes,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary_path).await;
                let _ = self.queue.lock().expect("transfer queue poisoned").fail(
                    transfer_id,
                    false,
                    SystemTime::now(),
                );
                self.persist(transfer_id).await?;
                return Err(error);
            }
        };
        let cache = Arc::clone(&self.cache);
        let record = tokio::task::spawn_blocking(move || {
            cache.lock().expect("cache manager poisoned").commit_file(
                connection_id,
                path,
                &temporary_path,
                size_bytes,
                pinned,
            )
        })
        .await
        .map_err(|error| TransferServiceError::CommitTask(error.to_string()))??;
        if let Some(store) = &self.store {
            store
                .save_cache(record.clone())
                .await
                .map_err(TransferServiceError::Persistence)?;
        }
        self.queue
            .lock()
            .expect("transfer queue poisoned")
            .complete(transfer_id)?;
        self.persist(transfer_id).await?;
        Ok(record.local_path)
    }

    pub async fn upload_cached(
        &self,
        provider: &dyn StorageProvider,
        connection_id: ConnectionId,
        path: RemotePath,
    ) -> Result<(), TransferServiceError> {
        let local_path = self
            .cache
            .lock()
            .expect("cache manager poisoned")
            .open(connection_id, &path)?;
        let size_bytes = tokio::fs::metadata(&local_path).await?.len();
        let transfer_id = {
            let mut queue = self.queue.lock().expect("transfer queue poisoned");
            let id = queue.enqueue(
                connection_id,
                path.clone(),
                TransferDirection::Upload,
                Some(size_bytes),
            );
            queue.start(id, SystemTime::now())?;
            id
        };
        self.persist(transfer_id).await?;
        let content = tokio_util::io::ReaderStream::new(tokio::fs::File::open(local_path).await?)
            .map(|chunk| chunk.map_err(StorageError::Io));
        let result = provider
            .write(bifrost_storage::WriteRequest {
                path,
                content: Box::pin(content),
                size_bytes: Some(size_bytes),
                modified_at: None,
            })
            .await;
        match result {
            Ok(_) => {
                self.queue
                    .lock()
                    .expect("transfer queue poisoned")
                    .update_progress(transfer_id, size_bytes)?;
                self.queue
                    .lock()
                    .expect("transfer queue poisoned")
                    .complete(transfer_id)?;
                self.persist(transfer_id).await?;
                Ok(())
            }
            Err(error) => {
                let _ = self.queue.lock().expect("transfer queue poisoned").fail(
                    transfer_id,
                    false,
                    SystemTime::now(),
                );
                self.persist(transfer_id).await?;
                Err(error.into())
            }
        }
    }

    async fn download_to_temp(
        &self,
        provider: &dyn StorageProvider,
        path: &RemotePath,
        temporary_path: &std::path::Path,
        transfer_id: TransferId,
    ) -> Result<u64, TransferServiceError> {
        let mut stream = provider
            .read(bifrost_storage::ReadRequest {
                path: path.clone(),
                range: None,
            })
            .await?;
        let mut file = tokio::fs::File::create(temporary_path).await?;
        let mut transferred = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            transferred += chunk.len() as u64;
            self.queue
                .lock()
                .expect("transfer queue poisoned")
                .update_progress(transfer_id, transferred)?;
            self.persist(transfer_id).await?;
        }
        tokio::io::AsyncWriteExt::shutdown(&mut file).await?;
        Ok(transferred)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TransferDirection, TransferQueue, TransferService, TransferSnapshot, TransferStatus,
        TransferStore,
    };
    use async_trait::async_trait;
    use bifrost_cache::CacheManager;
    use bifrost_common::{CapabilitySet, ConnectionId, ProviderKind, RemoteMetadata, RemotePath};
    use bifrost_storage::{
        ByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest,
    };
    use bytes::Bytes;
    use futures_util::stream;
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, SystemTime},
    };

    #[derive(Default)]
    struct MemoryStore(Mutex<Vec<TransferSnapshot>>);

    #[async_trait]
    impl TransferStore for MemoryStore {
        async fn save(&self, transfer: TransferSnapshot) -> Result<(), String> {
            self.0.lock().unwrap().push(transfer);
            Ok(())
        }

        async fn load_recoverable(&self) -> Result<Vec<TransferSnapshot>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    #[test]
    fn starts_only_within_the_concurrency_limit() {
        let mut queue = TransferQueue::new(1, 3);
        let first = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Download,
            Some(5),
        );
        let second = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Upload,
            None,
        );
        let started = queue.start_available(SystemTime::now());
        assert_eq!(started, vec![first]);
        assert_eq!(queue.get(second).unwrap().status, TransferStatus::Pending);
    }

    #[test]
    fn retry_uses_capped_exponential_backoff_and_eventually_fails() {
        let now = SystemTime::UNIX_EPOCH;
        let mut queue = TransferQueue::new(1, 2);
        let id = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Download,
            None,
        );
        queue.start_available(now);
        queue.fail(id, true, now).unwrap();
        assert_eq!(queue.get(id).unwrap().status, TransferStatus::Pending);
        assert_eq!(
            queue.get(id).unwrap().next_retry_at,
            Some(now + Duration::from_secs(2))
        );
        queue.start_available(now + Duration::from_secs(2));
        queue.fail(id, true, now + Duration::from_secs(2)).unwrap();
        assert_eq!(queue.get(id).unwrap().status, TransferStatus::Failed);
    }

    #[test]
    fn pause_resume_cancel_and_progress_are_explicit_state_changes() {
        let mut queue = TransferQueue::new(2, 1);
        let id = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Download,
            Some(10),
        );
        queue.start_available(SystemTime::now());
        queue.update_progress(id, 12).unwrap();
        assert_eq!(queue.get(id).unwrap().transferred_bytes, 10);
        queue.pause(id).unwrap();
        queue.resume(id).unwrap();
        queue.cancel(id).unwrap();
        assert_eq!(queue.get(id).unwrap().status, TransferStatus::Cancelled);
    }

    #[tokio::test]
    async fn recovers_running_transfers_as_pending() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        let connection_id = ConnectionId::new();
        let path = RemotePath::parse("recover.txt").unwrap();
        store.0.lock().unwrap().push(TransferSnapshot {
            id: bifrost_common::TransferId::new(),
            connection_id,
            path,
            direction: TransferDirection::Download,
            total_bytes: Some(10),
            transferred_bytes: 4,
            attempts: 1,
            status: TransferStatus::Running,
            next_retry_at: None,
        });
        let service = TransferService::with_store(
            CacheManager::new(directory.path().join("cache"), 100).unwrap(),
            1,
            2,
            Some(store),
        );

        assert_eq!(service.recover().await.unwrap(), 1);
        let queue = service.queue.lock().unwrap();
        assert_eq!(queue.jobs.len(), 1);
        assert_eq!(
            queue.jobs.values().next().unwrap().status,
            TransferStatus::Pending
        );
    }

    struct FakeProvider;

    #[async_trait]
    impl StorageProvider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::S3
        }

        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet::default()
        }

        async fn test_connection(&self) -> Result<(), StorageError> {
            Ok(())
        }

        async fn list(
            &self,
            _prefix: &RemotePath,
            _cursor: Option<&str>,
        ) -> Result<Page<RemoteEntry>, StorageError> {
            Ok(Page {
                entries: Vec::new(),
                next_cursor: None,
            })
        }

        async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
            Err(StorageError::NotFound { path: path.clone() })
        }

        async fn read(&self, _request: ReadRequest) -> Result<ByteStream, StorageError> {
            Ok(Box::pin(stream::iter(vec![Ok(Bytes::from_static(
                b"hydrated",
            ))])))
        }

        async fn write(&self, _request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
            Err(StorageError::Unsupported {
                provider: ProviderKind::S3,
                capability: "write".to_owned(),
            })
        }

        async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
            Err(StorageError::NotFound { path: path.clone() })
        }
    }

    #[tokio::test]
    async fn hydrate_streams_into_an_atomic_cache_file() {
        let directory = tempfile::tempdir().unwrap();
        let cache = CacheManager::new(directory.path().join("cache"), 1024).unwrap();
        let service = TransferService::new(cache, 1, 1);
        let path = RemotePath::parse("documents/report.txt").unwrap();
        let local_path = service
            .hydrate(&FakeProvider, ConnectionId::new(), path, Some(8), false)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(local_path).unwrap(), "hydrated");
        assert!(!directory.path().join("cache").join("partial").exists());
    }
}
