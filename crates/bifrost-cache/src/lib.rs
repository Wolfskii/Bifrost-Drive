use bifrost_common::{ConnectionId, RemotePath, TransferId};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("cache I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cache item is not available: {0}")]
    NotFound(RemotePath),
}

#[derive(Debug, Clone)]
pub struct CacheRecord {
    pub connection_id: ConnectionId,
    pub remote_path: RemotePath,
    pub local_path: PathBuf,
    pub size_bytes: u64,
    pub last_accessed: SystemTime,
    pub pinned: bool,
    pub active_transfer: bool,
}

#[derive(Debug)]
pub struct CacheManager {
    root: PathBuf,
    max_bytes: u64,
    records: HashMap<(ConnectionId, RemotePath), CacheRecord>,
}

impl CacheManager {
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Result<Self, CacheError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            max_bytes,
            records: HashMap::new(),
        })
    }

    pub fn cached_path(&self, connection_id: ConnectionId, remote_path: &RemotePath) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(connection_id.as_uuid().as_bytes());
        hasher.update([0]);
        hasher.update(remote_path.as_str().as_bytes());
        self.root.join(hex::encode(hasher.finalize()))
    }

    pub fn temporary_path(&self, transfer_id: TransferId) -> PathBuf {
        self.root
            .join(format!(".{}.partial", transfer_id.as_uuid()))
    }

    pub fn open(
        &mut self,
        connection_id: ConnectionId,
        remote_path: &RemotePath,
    ) -> Result<PathBuf, CacheError> {
        let key = (connection_id, remote_path.clone());
        let record = self
            .records
            .get_mut(&key)
            .ok_or_else(|| CacheError::NotFound(remote_path.clone()))?;
        if !record.local_path.is_file() {
            self.records.remove(&key);
            return Err(CacheError::NotFound(remote_path.clone()));
        }
        record.last_accessed = SystemTime::now();
        Ok(record.local_path.clone())
    }

    pub fn commit_file(
        &mut self,
        connection_id: ConnectionId,
        remote_path: RemotePath,
        temporary_path: &Path,
        size_bytes: u64,
        pinned: bool,
    ) -> Result<CacheRecord, CacheError> {
        let destination = self.cached_path(connection_id, &remote_path);
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        fs::rename(temporary_path, &destination)?;
        let record = CacheRecord {
            connection_id,
            remote_path: remote_path.clone(),
            local_path: destination,
            size_bytes,
            last_accessed: SystemTime::now(),
            pinned,
            active_transfer: false,
        };
        self.records
            .insert((connection_id, remote_path), record.clone());
        self.evict_until_within_limit()?;
        Ok(record)
    }

    pub fn set_pinned(
        &mut self,
        connection_id: ConnectionId,
        remote_path: &RemotePath,
        pinned: bool,
    ) -> Result<(), CacheError> {
        let record = self
            .records
            .get_mut(&(connection_id, remote_path.clone()))
            .ok_or_else(|| CacheError::NotFound(remote_path.clone()))?;
        record.pinned = pinned;
        self.evict_until_within_limit()
    }

    pub fn set_active_transfer(
        &mut self,
        connection_id: ConnectionId,
        remote_path: &RemotePath,
        active: bool,
    ) -> Result<(), CacheError> {
        let record = self
            .records
            .get_mut(&(connection_id, remote_path.clone()))
            .ok_or_else(|| CacheError::NotFound(remote_path.clone()))?;
        record.active_transfer = active;
        Ok(())
    }

    pub fn total_bytes(&self) -> u64 {
        self.records.values().map(|record| record.size_bytes).sum()
    }

    pub fn clear_unpinned(&mut self) -> Result<u64, CacheError> {
        let keys: Vec<_> = self
            .records
            .iter()
            .filter(|(_, record)| !record.pinned && !record.active_transfer)
            .map(|(key, _)| key.clone())
            .collect();
        self.remove_keys(keys)
    }

    fn evict_until_within_limit(&mut self) -> Result<(), CacheError> {
        while self.total_bytes() > self.max_bytes {
            let candidate = self
                .records
                .iter()
                .filter(|(_, record)| !record.pinned && !record.active_transfer)
                .min_by_key(|(_, record)| record.last_accessed)
                .map(|(key, _)| key.clone());
            let Some(candidate) = candidate else {
                break;
            };
            self.remove_keys(vec![candidate])?;
        }
        Ok(())
    }

    fn remove_keys(&mut self, keys: Vec<(ConnectionId, RemotePath)>) -> Result<u64, CacheError> {
        let mut removed = 0;
        for key in keys {
            if let Some(record) = self.records.remove(&key) {
                if record.local_path.exists() {
                    fs::remove_file(record.local_path)?;
                }
                removed += record.size_bytes;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::CacheManager;
    use bifrost_common::{ConnectionId, RemotePath};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn commits_to_a_stable_hashed_path_and_can_reopen() {
        let directory = tempdir().unwrap();
        let temporary = directory.path().join("partial");
        fs::write(&temporary, b"cached").unwrap();
        let connection = ConnectionId::new();
        let path = RemotePath::parse("docs/report.txt").unwrap();
        let mut cache = CacheManager::new(directory.path().join("cache"), 100).unwrap();

        let record = cache
            .commit_file(connection, path.clone(), &temporary, 6, false)
            .unwrap();
        assert!(!temporary.exists());
        assert_eq!(cache.open(connection, &path).unwrap(), record.local_path);
        assert!(
            record
                .local_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .len()
                >= 32
        );
    }

    #[test]
    fn evicts_oldest_unpinned_item_but_preserves_pins_and_active_transfers() {
        let directory = tempdir().unwrap();
        let mut cache = CacheManager::new(directory.path().join("cache"), 10).unwrap();
        let connection = ConnectionId::new();
        let first = RemotePath::parse("first").unwrap();
        let second = RemotePath::parse("second").unwrap();
        let first_temp = directory.path().join("first.partial");
        let second_temp = directory.path().join("second.partial");
        fs::write(&first_temp, b"123456").unwrap();
        fs::write(&second_temp, b"abcdef").unwrap();
        cache
            .commit_file(connection, first.clone(), &first_temp, 6, true)
            .unwrap();
        cache
            .commit_file(connection, second.clone(), &second_temp, 6, false)
            .unwrap();
        assert!(cache.open(connection, &first).is_ok());
        assert!(cache.open(connection, &second).is_err());
        assert_eq!(cache.total_bytes(), 6);

        let active_temp = directory.path().join("active.partial");
        fs::write(&active_temp, b"four").unwrap();
        let active = RemotePath::parse("active").unwrap();
        cache
            .commit_file(connection, active.clone(), &active_temp, 4, false)
            .unwrap();
        cache
            .set_active_transfer(connection, &active, true)
            .unwrap();
        assert!(cache.open(connection, &active).is_ok());
    }
}
