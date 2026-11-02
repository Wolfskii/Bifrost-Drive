use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use async_trait::async_trait;
use bifrost_common::{CapabilitySet, ProviderKind, RemoteMetadata};
use bifrost_storage::{ByteStream, Page, RemoteEntry};
use bytes::Bytes;
use futures_util::stream;

use super::*;

#[derive(Default)]
struct MemoryProvider {
    files: Mutex<HashMap<String, Vec<u8>>>,
    dirs: Mutex<HashSet<String>>,
    deny_write: bool,
    lie_about_size: bool,
    server_side_rename: bool,
    write_delay: Duration,
    read_delay: Duration,
    delete_delay: Duration,
    /// Deleting a path that contains this text is refused.
    refuse_delete: Option<String>,
    write_order: Mutex<Vec<String>>,
    network_failures: Mutex<usize>,
    active: AtomicUsize,
    peak: AtomicUsize,
    stat_calls: AtomicUsize,
    list_calls: AtomicUsize,
    write_calls: AtomicUsize,
}

impl MemoryProvider {
    fn with(files: &[(&str, &[u8])], dirs: &[&str]) -> Arc<Self> {
        Self::build(files, dirs, |_| {})
    }

    fn build(
        files: &[(&str, &[u8])],
        dirs: &[&str],
        configure: impl FnOnce(&mut Self),
    ) -> Arc<Self> {
        let mut provider = Self::default();
        for (path, content) in files {
            provider
                .files
                .get_mut()
                .unwrap()
                .insert((*path).to_owned(), content.to_vec());
        }
        for dir in dirs {
            provider.dirs.get_mut().unwrap().insert((*dir).to_owned());
        }
        configure(&mut provider);
        Arc::new(provider)
    }

    fn content(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    fn has_file(&self, path: &str) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }

    fn parent(path: &str) -> &str {
        path.rsplit_once('/').map_or("", |(parent, _)| parent)
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
        let prefix = prefix.as_str();
        let mut entries = Vec::new();
        for (path, content) in self.files.lock().unwrap().iter() {
            if Self::parent(path) == prefix {
                entries.push(RemoteEntry {
                    metadata: RemoteMetadata {
                        path: RemotePath::parse(path).unwrap(),
                        is_directory: false,
                        size_bytes: Some(content.len() as u64),
                        etag: None,
                        modified_at: None,
                    },
                });
            }
        }
        for path in self.dirs.lock().unwrap().iter() {
            if Self::parent(path) == prefix && !path.is_empty() {
                entries.push(RemoteEntry {
                    metadata: RemoteMetadata {
                        path: RemotePath::parse(path).unwrap(),
                        is_directory: true,
                        size_bytes: None,
                        etag: None,
                        modified_at: None,
                    },
                });
            }
        }
        Ok(Page {
            entries,
            next_cursor: None,
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        self.stat_calls.fetch_add(1, Ordering::Relaxed);
        let key = path.as_str();
        let size = self
            .files
            .lock()
            .unwrap()
            .get(key)
            .map(|data| data.len() as u64 + u64::from(self.lie_about_size));
        if let Some(size) = size {
            return Ok(RemoteMetadata {
                path: path.clone(),
                is_directory: false,
                size_bytes: Some(size),
                etag: None,
                modified_at: None,
            });
        }
        if key.is_empty() || self.dirs.lock().unwrap().contains(key) {
            return Ok(RemoteMetadata {
                path: path.clone(),
                is_directory: true,
                size_bytes: None,
                etag: None,
                modified_at: None,
            });
        }
        Err(StorageError::NotFound { path: path.clone() })
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let data = self
            .content(request.path.as_str())
            .ok_or_else(|| StorageError::NotFound {
                path: request.path.clone(),
            })?;
        let delay = self.read_delay;
        let chunks = data
            .chunks(1000)
            .map(Bytes::copy_from_slice)
            .collect::<Vec<_>>();
        Ok(Box::pin(stream::iter(chunks).then(
            move |chunk| async move {
                tokio::time::sleep(delay).await;
                Ok(chunk)
            },
        )))
    }

    async fn write(&self, mut request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        self.write_calls.fetch_add(1, Ordering::Relaxed);
        self.write_order
            .lock()
            .unwrap()
            .push(request.path.as_str().to_owned());
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let result = async {
            if self.deny_write {
                return Err(StorageError::PermissionDenied {
                    path: request.path.clone(),
                });
            }
            {
                let mut failures = self.network_failures.lock().unwrap();
                if *failures > 0 {
                    *failures -= 1;
                    return Err(StorageError::Network {
                        provider: ProviderKind::Sftp,
                        message: "connection reset".to_owned(),
                    });
                }
            }
            let mut data = Vec::new();
            while let Some(chunk) = request.content.next().await {
                data.extend_from_slice(&chunk?);
            }
            tokio::time::sleep(self.write_delay).await;
            let size = data.len() as u64;
            self.files
                .lock()
                .unwrap()
                .insert(request.path.as_str().to_owned(), data);
            Ok(RemoteMetadata {
                path: request.path.clone(),
                is_directory: false,
                size_bytes: Some(size),
                etag: None,
                modified_at: None,
            })
        }
        .await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(self.delete_delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self
            .refuse_delete
            .as_deref()
            .is_some_and(|text| path.as_str().contains(text))
        {
            return Err(StorageError::PermissionDenied { path: path.clone() });
        }
        self.files.lock().unwrap().remove(path.as_str());
        self.dirs.lock().unwrap().remove(path.as_str());
        Ok(())
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        self.dirs.lock().unwrap().insert(path.as_str().to_owned());
        Ok(())
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        if !self.server_side_rename {
            return Err(StorageError::Unsupported {
                provider: ProviderKind::Sftp,
                capability: "rename".to_owned(),
            });
        }
        let mut files = self.files.lock().unwrap();
        let data = files
            .remove(from.as_str())
            .ok_or_else(|| StorageError::NotFound { path: from.clone() })?;
        files.insert(to.as_str().to_owned(), data);
        Ok(())
    }
}

fn options() -> BatchOptions {
    BatchOptions {
        retry_base_delay: Duration::from_millis(1),
        ..BatchOptions::default()
    }
}

fn remote(provider: &Arc<MemoryProvider>, path: &str) -> Endpoint {
    Endpoint::Remote {
        provider: Arc::clone(provider) as Arc<dyn StorageProvider>,
        path: RemotePath::parse(path).unwrap(),
    }
}

fn write_local(root: &std::path::Path, files: &[(&str, &[u8])]) {
    for (path, content) in files {
        let target = root.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
}

async fn run_batch(
    sources: Vec<Endpoint>,
    destination: Endpoint,
    mode: TransferMode,
    options: BatchOptions,
) -> BatchSnapshot {
    start(
        BatchRequest {
            sources,
            destination,
            mode,
        },
        options,
    )
    .wait()
    .await
}

async fn wait_for_state(handle: &BatchHandle, state: BatchState) {
    handle
        .subscribe()
        .wait_for(|snapshot| snapshot.state == state)
        .await
        .unwrap();
}

#[tokio::test]
async fn uploads_a_folder_tree_and_reports_totals() {
    let local = tempfile::tempdir().unwrap();
    write_local(
        local.path(),
        &[
            ("photos/a.jpg", b"aaaa"),
            ("photos/trip/b.jpg", b"bb"),
            ("photos/trip/c.jpg", b"c"),
        ],
    );
    write_local(local.path(), &[("notes.txt", b"hello")]);
    let provider = MemoryProvider::with(&[], &["dest"]);

    let finished = run_batch(
        vec![
            Endpoint::Local(local.path().join("photos")),
            Endpoint::Local(local.path().join("notes.txt")),
        ],
        remote(&provider, "dest"),
        TransferMode::Copy,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(finished.total_files, 4);
    assert_eq!(finished.done_files, 4);
    assert_eq!(finished.total_bytes, 12);
    assert_eq!(finished.done_bytes, 12);
    assert_eq!(provider.content("dest/photos/a.jpg").unwrap(), b"aaaa");
    assert_eq!(provider.content("dest/photos/trip/b.jpg").unwrap(), b"bb");
    assert_eq!(provider.content("dest/notes.txt").unwrap(), b"hello");
    assert!(local.path().join("photos/a.jpg").exists());
}

#[tokio::test]
async fn downloads_a_folder_tree_without_leaving_partial_files() {
    let provider = MemoryProvider::with(
        &[("src/a.txt", b"one"), ("src/deep/b.txt", b"two")],
        &["src", "src/deep"],
    );
    let local = tempfile::tempdir().unwrap();

    let finished = run_batch(
        vec![remote(&provider, "src")],
        Endpoint::Local(local.path().to_path_buf()),
        TransferMode::Copy,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(
        std::fs::read(local.path().join("src/a.txt")).unwrap(),
        b"one"
    );
    assert_eq!(
        std::fs::read(local.path().join("src/deep/b.txt")).unwrap(),
        b"two"
    );
    let leftovers = walk(local.path())
        .into_iter()
        .filter(|name| name.contains("bifrost-part"))
        .count();
    assert_eq!(leftovers, 0);
}

fn walk(root: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        names.push(entry.file_name().to_string_lossy().into_owned());
        if entry.file_type().unwrap().is_dir() {
            names.extend(walk(&entry.path()));
        }
    }
    names
}

#[tokio::test]
async fn never_runs_more_transfers_at_once_than_allowed() {
    let local = tempfile::tempdir().unwrap();
    for number in 0..24 {
        write_local(local.path(), &[(&format!("f{number}.bin"), b"data")]);
    }
    let provider = MemoryProvider::build(&[], &["dest"], |provider| {
        provider.write_delay = Duration::from_millis(15);
    });
    let sources = (0..24)
        .map(|number| Endpoint::Local(local.path().join(format!("f{number}.bin"))))
        .collect();

    let finished = run_batch(
        sources,
        remote(&provider, "dest"),
        TransferMode::Copy,
        BatchOptions {
            max_concurrent: 3,
            ..options()
        },
    )
    .await;

    assert_eq!(finished.done_files, 24);
    let peak = provider.peak.load(Ordering::SeqCst);
    assert!(peak <= 3, "peak concurrency was {peak}");
    assert!(peak > 1, "transfers never overlapped");
}

#[tokio::test]
async fn move_removes_the_sources_after_a_successful_upload() {
    let local = tempfile::tempdir().unwrap();
    write_local(
        local.path(),
        &[("clips/a.mkv", b"aaaa"), ("clips/more/b.mkv", b"bb")],
    );
    let provider = MemoryProvider::with(&[], &["dest"]);

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("clips"))],
        remote(&provider, "dest"),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(provider.content("dest/clips/more/b.mkv").unwrap(), b"bb");
    assert!(!local.path().join("clips").exists());
}

#[tokio::test]
async fn move_keeps_every_source_when_the_server_refuses_writes() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("clips/a.mkv", b"aaaa")]);
    let provider = MemoryProvider::build(&[], &["dest"], |provider| provider.deny_write = true);

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("clips"))],
        remote(&provider, "dest"),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::CompletedWithErrors);
    assert_eq!(finished.failed_files, 1);
    assert_eq!(
        finished.failures,
        vec![ItemFailure {
            name: "clips/a.mkv".to_owned(),
            reason: FailureReason::PermissionDenied,
        }]
    );
    assert_eq!(
        std::fs::read(local.path().join("clips/a.mkv")).unwrap(),
        b"aaaa"
    );
}

#[tokio::test]
async fn move_keeps_the_source_when_the_uploaded_size_does_not_match() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.mkv", b"aaaa")]);
    let provider = MemoryProvider::build(&[], &["dest"], |provider| provider.lie_about_size = true);

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("a.mkv"))],
        remote(&provider, "dest"),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.failed_files, 1);
    assert!(local.path().join("a.mkv").exists());
}

#[tokio::test]
async fn remote_move_keeps_the_source_when_the_download_cannot_be_stored() {
    let provider = MemoryProvider::with(&[("src/a.txt", b"one")], &["src"]);
    let local = tempfile::tempdir().unwrap();
    // A file where a folder is needed makes the local write fail.
    std::fs::write(local.path().join("src"), b"in the way").unwrap();

    let finished = run_batch(
        vec![remote(&provider, "src")],
        Endpoint::Local(local.path().to_path_buf()),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::CompletedWithErrors);
    assert!(provider.has_file("src/a.txt"));
}

#[tokio::test]
async fn replacing_a_clashing_file_waits_for_the_answer() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"new")]);
    let provider = MemoryProvider::with(&[("dest/a.txt", b"old")], &["dest"]);
    let handle = start(
        BatchRequest {
            sources: vec![Endpoint::Local(local.path().join("a.txt"))],
            destination: remote(&provider, "dest"),
            mode: TransferMode::Copy,
        },
        options(),
    );

    wait_for_state(&handle, BatchState::WaitingForAnswer).await;
    assert_eq!(
        handle.snapshot().conflict,
        Some(ConflictPrompt {
            name: "a.txt".to_owned(),
            source_bytes: 3,
            destination_bytes: 3,
        })
    );
    assert_eq!(provider.content("dest/a.txt").unwrap(), b"old");
    handle.answer_conflict(Resolution::Replace, false);

    assert_eq!(handle.wait().await.state, BatchState::Completed);
    assert_eq!(provider.content("dest/a.txt").unwrap(), b"new");
}

#[tokio::test]
async fn skipping_a_clash_leaves_both_files_untouched() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"new")]);
    let provider = MemoryProvider::with(&[("dest/a.txt", b"old")], &["dest"]);
    let handle = start(
        BatchRequest {
            sources: vec![Endpoint::Local(local.path().join("a.txt"))],
            destination: remote(&provider, "dest"),
            mode: TransferMode::Move,
        },
        options(),
    );

    wait_for_state(&handle, BatchState::WaitingForAnswer).await;
    handle.answer_conflict(Resolution::Skip, false);
    let finished = handle.wait().await;

    assert_eq!(finished.skipped_files, 1);
    assert_eq!(provider.content("dest/a.txt").unwrap(), b"old");
    assert!(local.path().join("a.txt").exists());
}

#[tokio::test]
async fn keeping_both_writes_a_second_name() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"new")]);
    let provider = MemoryProvider::with(
        &[("dest/a.txt", b"old"), ("dest/a (2).txt", b"older")],
        &["dest"],
    );
    let handle = start(
        BatchRequest {
            sources: vec![Endpoint::Local(local.path().join("a.txt"))],
            destination: remote(&provider, "dest"),
            mode: TransferMode::Copy,
        },
        options(),
    );

    wait_for_state(&handle, BatchState::WaitingForAnswer).await;
    handle.answer_conflict(Resolution::KeepBoth, false);
    handle.wait().await;

    assert_eq!(provider.content("dest/a.txt").unwrap(), b"old");
    assert_eq!(provider.content("dest/a (2).txt").unwrap(), b"older");
    assert_eq!(provider.content("dest/a (3).txt").unwrap(), b"new");
}

#[tokio::test]
async fn one_answer_can_cover_every_clash() {
    let local = tempfile::tempdir().unwrap();
    write_local(
        local.path(),
        &[("a.txt", b"A"), ("b.txt", b"B"), ("c.txt", b"C")],
    );
    let provider = MemoryProvider::with(
        &[
            ("dest/a.txt", b"x"),
            ("dest/b.txt", b"x"),
            ("dest/c.txt", b"x"),
        ],
        &["dest"],
    );
    let handle = start(
        BatchRequest {
            sources: ["a.txt", "b.txt", "c.txt"]
                .iter()
                .map(|name| Endpoint::Local(local.path().join(name)))
                .collect(),
            destination: remote(&provider, "dest"),
            mode: TransferMode::Copy,
        },
        options(),
    );

    wait_for_state(&handle, BatchState::WaitingForAnswer).await;
    handle.answer_conflict(Resolution::Replace, true);
    let finished = handle.wait().await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(provider.content("dest/b.txt").unwrap(), b"B");
    assert_eq!(provider.content("dest/c.txt").unwrap(), b"C");
}

#[tokio::test]
async fn cancelling_stops_a_download_and_removes_its_partial_file() {
    let provider = MemoryProvider::build(&[("big.bin", &[7u8; 20_000])], &[], |provider| {
        provider.read_delay = Duration::from_millis(40);
    });
    let local = tempfile::tempdir().unwrap();
    let handle = start(
        BatchRequest {
            sources: vec![remote(&provider, "big.bin")],
            destination: Endpoint::Local(local.path().to_path_buf()),
            mode: TransferMode::Copy,
        },
        options(),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;
    handle.cancel();
    let finished = handle.wait().await;

    assert_eq!(finished.state, BatchState::Cancelled);
    assert!(walk(local.path()).is_empty());
}

#[tokio::test]
async fn network_errors_are_retried_before_the_file_is_reported() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"data")]);
    let provider = MemoryProvider::build(&[], &["dest"], |provider| {
        *provider.network_failures.get_mut().unwrap() = 2;
    });

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("a.txt"))],
        remote(&provider, "dest"),
        TransferMode::Copy,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(finished.done_bytes, 4);
    assert_eq!(provider.write_calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn a_file_that_keeps_failing_is_reported_with_the_network_reason() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"data")]);
    let provider = MemoryProvider::build(&[], &["dest"], |provider| {
        *provider.network_failures.get_mut().unwrap() = 10;
    });

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("a.txt"))],
        remote(&provider, "dest"),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::CompletedWithErrors);
    assert_eq!(
        finished.failures[0].reason,
        FailureReason::Network("connection reset".to_owned())
    );
    assert!(local.path().join("a.txt").exists());
}

#[tokio::test]
async fn pausing_holds_new_files_back_until_resumed() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"a"), ("b.txt", b"b")]);
    let provider = MemoryProvider::with(&[], &["dest"]);
    let handle = start(
        BatchRequest {
            sources: ["a.txt", "b.txt"]
                .iter()
                .map(|name| Endpoint::Local(local.path().join(name)))
                .collect(),
            destination: remote(&provider, "dest"),
            mode: TransferMode::Copy,
        },
        options(),
    );
    handle.pause();

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(handle.snapshot().state, BatchState::Paused);
    assert_eq!(provider.write_calls.load(Ordering::Relaxed), 0);

    handle.resume();
    assert_eq!(handle.wait().await.state, BatchState::Completed);
    assert_eq!(provider.write_calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn busy_existing_folders_are_listed_once_instead_of_checked_per_file() {
    let local = tempfile::tempdir().unwrap();
    let names = (0..30)
        .map(|number| format!("f{number}.txt"))
        .collect::<Vec<_>>();
    for name in &names {
        write_local(local.path(), &[(name, b"x")]);
    }
    let provider = MemoryProvider::with(&[("dest/existing.txt", b"x")], &["dest"]);

    let finished = run_batch(
        names
            .iter()
            .map(|name| Endpoint::Local(local.path().join(name)))
            .collect(),
        remote(&provider, "dest"),
        TransferMode::Copy,
        options(),
    )
    .await;

    assert_eq!(finished.done_files, 30);
    assert_eq!(provider.list_calls.load(Ordering::Relaxed), 1);
    // Only the destination folder itself is checked.
    assert_eq!(provider.stat_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn moving_inside_one_server_renames_instead_of_copying_data() {
    let provider = MemoryProvider::build(&[("a/x.bin", b"data")], &["a", "b"], |provider| {
        provider.server_side_rename = true;
    });

    let finished = run_batch(
        vec![remote(&provider, "a/x.bin")],
        remote(&provider, "b"),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(provider.content("b/x.bin").unwrap(), b"data");
    assert!(!provider.has_file("a/x.bin"));
    assert_eq!(provider.write_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn copying_between_two_servers_goes_through_a_staged_file() {
    let from = MemoryProvider::with(&[("src/a.bin", &[9u8; 3000])], &["src"]);
    let to = MemoryProvider::with(&[], &["dest"]);
    let staging = tempfile::tempdir().unwrap();

    let finished = run_batch(
        vec![remote(&from, "src")],
        remote(&to, "dest"),
        TransferMode::Move,
        BatchOptions {
            temp_dir: staging.path().to_path_buf(),
            ..options()
        },
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(finished.done_bytes, 3000);
    assert_eq!(to.content("dest/src/a.bin").unwrap(), vec![9u8; 3000]);
    assert!(!from.has_file("src/a.bin"));
    assert!(walk(staging.path()).is_empty());
}

#[tokio::test]
async fn a_destination_that_is_not_a_folder_fails_the_batch() {
    let local = tempfile::tempdir().unwrap();
    write_local(local.path(), &[("a.txt", b"a")]);
    let provider = MemoryProvider::with(&[("dest", b"i am a file")], &[]);

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("a.txt"))],
        remote(&provider, "dest"),
        TransferMode::Copy,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Failed);
    assert_eq!(finished.done_files, 0);
}

#[tokio::test]
async fn thousands_of_small_files_finish_with_exact_totals() {
    let local = tempfile::tempdir().unwrap();
    for number in 0..300 {
        write_local(
            local.path(),
            &[(&format!("many/{}/f{number}.txt", number % 10), b"12345")],
        );
    }
    let provider = MemoryProvider::with(&[], &["dest"]);

    let finished = run_batch(
        vec![Endpoint::Local(local.path().join("many"))],
        remote(&provider, "dest"),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(finished.done_files, 300);
    assert_eq!(finished.done_bytes, 1500);
    assert!(!local.path().join("many").exists());
}

#[tokio::test]
async fn copies_a_folder_between_two_local_folders_and_keeps_the_originals() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    write_local(
        from.path(),
        &[("clips/a.mkv", b"aaaa"), ("clips/more/b.mkv", b"bb")],
    );
    let modified = std::fs::metadata(from.path().join("clips/a.mkv"))
        .unwrap()
        .modified()
        .unwrap();

    let finished = run_batch(
        vec![Endpoint::Local(from.path().join("clips"))],
        Endpoint::Local(to.path().to_path_buf()),
        TransferMode::Copy,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(finished.done_files, 2);
    assert_eq!(
        std::fs::read(to.path().join("clips/a.mkv")).unwrap(),
        b"aaaa"
    );
    assert_eq!(
        std::fs::read(to.path().join("clips/more/b.mkv")).unwrap(),
        b"bb"
    );
    assert_eq!(
        std::fs::metadata(to.path().join("clips/a.mkv"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert!(from.path().join("clips/a.mkv").exists());
    assert!(walk(to.path())
        .iter()
        .all(|name| !name.contains("bifrost-part")));
}

#[tokio::test]
async fn moves_a_folder_between_two_local_folders_and_removes_the_original() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    write_local(
        from.path(),
        &[("clips/a.mkv", b"aaaa"), ("clips/more/b.mkv", b"bb")],
    );

    let finished = run_batch(
        vec![Endpoint::Local(from.path().join("clips"))],
        Endpoint::Local(to.path().to_path_buf()),
        TransferMode::Move,
        options(),
    )
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(
        std::fs::read(to.path().join("clips/a.mkv")).unwrap(),
        b"aaaa"
    );
    assert_eq!(
        std::fs::read(to.path().join("clips/more/b.mkv")).unwrap(),
        b"bb"
    );
    assert!(!from.path().join("clips").exists());
}

#[tokio::test]
async fn replacing_a_file_with_itself_never_loses_it() {
    let folder = tempfile::tempdir().unwrap();
    write_local(folder.path(), &[("a.txt", b"precious")]);
    let handle = start(
        BatchRequest {
            sources: vec![Endpoint::Local(folder.path().join("a.txt"))],
            destination: Endpoint::Local(folder.path().to_path_buf()),
            mode: TransferMode::Move,
        },
        options(),
    );

    wait_for_state(&handle, BatchState::WaitingForAnswer).await;
    handle.answer_conflict(Resolution::Replace, false);
    let finished = handle.wait().await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(
        std::fs::read(folder.path().join("a.txt")).unwrap(),
        b"precious"
    );
}

#[tokio::test]
async fn keeping_both_when_pasting_into_the_same_local_folder_makes_a_second_name() {
    let folder = tempfile::tempdir().unwrap();
    write_local(folder.path(), &[("a.txt", b"one")]);
    let handle = start(
        BatchRequest {
            sources: vec![Endpoint::Local(folder.path().join("a.txt"))],
            destination: Endpoint::Local(folder.path().to_path_buf()),
            mode: TransferMode::Copy,
        },
        options(),
    );

    wait_for_state(&handle, BatchState::WaitingForAnswer).await;
    handle.answer_conflict(Resolution::KeepBoth, false);
    handle.wait().await;

    assert_eq!(std::fs::read(folder.path().join("a.txt")).unwrap(), b"one");
    assert_eq!(
        std::fs::read(folder.path().join("a (2).txt")).unwrap(),
        b"one"
    );
}

fn big_tree(provider_files: &mut Vec<(String, Vec<u8>)>, folders: &[&str], per_folder: usize) {
    for folder in folders {
        for number in 0..per_folder {
            provider_files.push((format!("{folder}/f{number}.dat"), vec![7u8; 10]));
        }
    }
}

#[tokio::test]
async fn deletes_a_remote_tree_in_parallel_and_removes_the_emptied_folders() {
    let mut files = Vec::new();
    big_tree(
        &mut files,
        &["photos", "photos/trip", "photos/trip/day1"],
        15,
    );
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let provider = MemoryProvider::build(
        &borrowed,
        &["photos", "photos/trip", "photos/trip/day1"],
        |provider| provider.delete_delay = Duration::from_millis(10),
    );

    let finished = start_delete(
        DeleteRequest {
            targets: vec![remote(&provider, "photos")],
        },
        BatchOptions {
            max_concurrent: 8,
            ..options()
        },
    )
    .wait()
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert_eq!(finished.mode, TransferMode::Delete);
    assert_eq!(finished.total_files, 45);
    assert_eq!(finished.done_files, 45);
    assert!(provider.files.lock().unwrap().is_empty());
    assert!(provider.dirs.lock().unwrap().is_empty());
    let peak = provider.peak.load(Ordering::SeqCst);
    assert!(peak > 1 && peak <= 8, "peak concurrency was {peak}");
}

#[tokio::test]
async fn a_file_that_cannot_be_deleted_is_reported_and_keeps_its_folder() {
    let provider = MemoryProvider::build(
        &[
            ("docs/a.txt", b"a"),
            ("docs/keep.txt", b"k"),
            ("docs/sub/b.txt", b"b"),
        ],
        &["docs", "docs/sub"],
        |provider| provider.refuse_delete = Some("keep.txt".to_owned()),
    );

    let finished = start_delete(
        DeleteRequest {
            targets: vec![remote(&provider, "docs")],
        },
        options(),
    )
    .wait()
    .await;

    assert_eq!(finished.state, BatchState::CompletedWithErrors);
    assert_eq!(finished.failed_files, 1);
    assert_eq!(finished.failures[0].name, "docs/keep.txt");
    assert_eq!(finished.failures[0].reason, FailureReason::PermissionDenied);
    assert!(provider.has_file("docs/keep.txt"));
    assert!(!provider.has_file("docs/a.txt"));
    assert!(!provider.has_file("docs/sub/b.txt"));
    assert!(provider.dirs.lock().unwrap().contains("docs"));
    assert!(!provider.dirs.lock().unwrap().contains("docs/sub"));
}

#[tokio::test]
async fn deleting_something_that_is_already_gone_is_reported() {
    let provider = MemoryProvider::with(&[], &[]);

    let finished = start_delete(
        DeleteRequest {
            targets: vec![remote(&provider, "missing.txt")],
        },
        options(),
    )
    .wait()
    .await;

    assert_eq!(finished.failed_files, 1);
    assert_eq!(finished.failures[0].reason, FailureReason::NotFound);
}

#[tokio::test]
async fn deletes_local_folders_and_single_files() {
    let local = tempfile::tempdir().unwrap();
    write_local(
        local.path(),
        &[
            ("tree/a.txt", b"a"),
            ("tree/deep/er/b.txt", b"b"),
            ("single.txt", b"s"),
            ("untouched/c.txt", b"c"),
        ],
    );

    let finished = start_delete(
        DeleteRequest {
            targets: vec![
                Endpoint::Local(local.path().join("tree")),
                Endpoint::Local(local.path().join("single.txt")),
            ],
        },
        options(),
    )
    .wait()
    .await;

    assert_eq!(finished.state, BatchState::Completed);
    assert!(!local.path().join("tree").exists());
    assert!(!local.path().join("single.txt").exists());
    assert!(local.path().join("untouched/c.txt").exists());
}

#[tokio::test]
async fn cancelling_a_delete_stops_before_the_rest_are_removed() {
    let mut files = Vec::new();
    big_tree(&mut files, &["bulk"], 60);
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let provider = MemoryProvider::build(&borrowed, &["bulk"], |provider| {
        provider.delete_delay = Duration::from_millis(20);
    });
    let handle = start_delete(
        DeleteRequest {
            targets: vec![remote(&provider, "bulk")],
        },
        BatchOptions {
            max_concurrent: 2,
            ..options()
        },
    );

    handle
        .subscribe()
        .wait_for(|snapshot| snapshot.done_files >= 2)
        .await
        .unwrap();
    handle.cancel();
    let finished = handle.wait().await;

    assert_eq!(finished.state, BatchState::Cancelled);
    assert!(!provider.files.lock().unwrap().is_empty());
    assert!(provider.dirs.lock().unwrap().contains("bulk"));
}

#[tokio::test]
async fn ordered_batches_write_in_path_order() {
    let local = tempfile::tempdir().unwrap();
    write_local(
        local.path(),
        &[
            ("set/z.txt", b"z"),
            ("set/b/y.txt", b"y"),
            ("set/a.txt", b"a"),
            ("set/b/c.txt", b"c"),
            ("set/m.txt", b"m"),
        ],
    );
    let provider = MemoryProvider::with(&[], &["dest"]);

    run_batch(
        vec![Endpoint::Local(local.path().join("set"))],
        remote(&provider, "dest"),
        TransferMode::Copy,
        BatchOptions {
            max_concurrent: 1,
            ordered: true,
            ..options()
        },
    )
    .await;

    let order = provider.write_order.lock().unwrap().clone();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order.len(), 5);
    assert_eq!(order, sorted);
}

#[tokio::test]
async fn an_adaptive_batch_raises_parallelism_while_the_server_keeps_up() {
    let local = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..160).map(|number| format!("f{number}.bin")).collect();
    for name in &names {
        write_local(local.path(), &[(name.as_str(), b"data")]);
    }
    let provider = MemoryProvider::build(&[], &["dest"], |provider| {
        provider.write_delay = Duration::from_millis(20);
    });

    let finished = run_batch(
        names
            .iter()
            .map(|name| Endpoint::Local(local.path().join(name)))
            .collect(),
        remote(&provider, "dest"),
        TransferMode::Copy,
        BatchOptions {
            max_concurrent: 1,
            adaptive_ceiling: 8,
            tune_interval: Duration::from_millis(60),
            ..options()
        },
    )
    .await;

    assert_eq!(finished.done_files, 160);
    let peak = provider.peak.load(Ordering::SeqCst);
    assert!(peak > 2, "parallelism never grew (peak {peak})");
    assert!(peak <= 8, "parallelism passed its ceiling (peak {peak})");
}

#[tokio::test]
async fn the_ceiling_never_passes_the_hard_cap() {
    let local = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..120).map(|number| format!("f{number}.bin")).collect();
    for name in &names {
        write_local(local.path(), &[(name.as_str(), b"data")]);
    }
    let provider = MemoryProvider::build(&[], &["dest"], |provider| {
        provider.write_delay = Duration::from_millis(20);
    });

    run_batch(
        names
            .iter()
            .map(|name| Endpoint::Local(local.path().join(name)))
            .collect(),
        remote(&provider, "dest"),
        TransferMode::Copy,
        BatchOptions {
            max_concurrent: 4,
            adaptive_ceiling: 500,
            tune_interval: Duration::from_millis(40),
            ..options()
        },
    )
    .await;

    assert!(provider.peak.load(Ordering::SeqCst) <= 16);
}
