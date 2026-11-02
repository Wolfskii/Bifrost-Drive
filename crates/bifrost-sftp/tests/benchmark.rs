//! Throughput of the batch engine over a real SFTP connection, one file at a time (how Explorer
//! works) against several files at once. Needs the Docker SFTP server, so it is ignored by
//! default. Run it with
//!
//! ```text
//! BIFROST_SFTP_INTEGRATION=1 cargo test -p bifrost-sftp --release --test benchmark -- --ignored --nocapture
//! ```
//!
//! `BIFROST_BENCH_FILES` (default 1000) and `BIFROST_BENCH_FILE_KB` (default 16) set the
//! workload. The server runs on this machine, so round trips are about as cheap as they get;
//! a real server across a network is slower per file and gains more from parallelism.

use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use bifrost_common::RemotePath;
use bifrost_sftp::{SftpConfig, SftpProvider};
use bifrost_storage::StorageProvider;
use bifrost_transfer::batch::{
    start, start_delete, BatchOptions, BatchRequest, BatchState, DeleteRequest, Endpoint,
    TransferMode,
};

fn setting(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn provider() -> Arc<dyn StorageProvider> {
    let known_hosts = env::var("BIFROST_SFTP_KNOWN_HOSTS").unwrap_or_else(|_| {
        format!(
            "{}/../../.cache/sftp_known_hosts",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    Arc::new(
        SftpProvider::connect(
            SftpConfig {
                host: "127.0.0.1".to_owned(),
                port: env::var("SFTP_PORT")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(2222),
                username: "bifrost-dev".to_owned(),
                known_hosts: PathBuf::from(known_hosts),
                root_path: String::new(),
                trust_on_first_use: false,
            },
            "bifrost-dev-secret",
        )
        .unwrap(),
    )
}

fn make_tree(root: &Path, files: usize, kilobytes: usize) {
    let content = vec![0x5Au8; kilobytes * 1024];
    let folders = (files / 50).max(1);
    for folder in 0..folders {
        std::fs::create_dir_all(root.join(format!("set/folder{folder:04}"))).unwrap();
    }
    for number in 0..files {
        std::fs::write(
            root.join(format!(
                "set/folder{:04}/file{number:06}.dat",
                number % folders
            )),
            &content,
        )
        .unwrap();
    }
}

fn tuned(start: usize, ceiling: usize) -> BatchOptions {
    BatchOptions {
        max_concurrent: start,
        adaptive_ceiling: ceiling,
        ..BatchOptions::default()
    }
}

fn report(label: &str, files: usize, took: Duration) {
    println!(
        "{label:<44} {:>8.2} s {:>10.0} files/s",
        took.as_secs_f64(),
        files as f64 / took.as_secs_f64()
    );
}

async fn make_remote_dir(provider: &Arc<dyn StorageProvider>, path: &str) -> Endpoint {
    let path = RemotePath::parse(path).unwrap();
    provider.create_directory(&path).await.unwrap();
    Endpoint::Remote {
        provider: Arc::clone(provider),
        path,
    }
}

async fn timed_copy(
    sources: Vec<Endpoint>,
    destination: Endpoint,
    options: BatchOptions,
) -> Duration {
    let started = Instant::now();
    let finished = start(
        BatchRequest {
            sources,
            destination,
            mode: TransferMode::Copy,
        },
        options,
    )
    .wait()
    .await;
    assert_eq!(finished.state, BatchState::Completed, "{finished:?}");
    started.elapsed()
}

async fn timed_delete(target: Endpoint, options: BatchOptions) -> Duration {
    let started = Instant::now();
    let finished = start_delete(
        DeleteRequest {
            targets: vec![target],
        },
        options,
    )
    .wait()
    .await;
    assert_eq!(finished.state, BatchState::Completed, "{finished:?}");
    started.elapsed()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Docker SFTP server (task docker:up) and BIFROST_SFTP_INTEGRATION=1"]
async fn upload_download_and_delete_over_sftp() {
    if env::var_os("BIFROST_SFTP_INTEGRATION").is_none() {
        eprintln!("BIFROST_SFTP_INTEGRATION is not set; skipping");
        return;
    }
    let files = setting("BIFROST_BENCH_FILES", 1000);
    let kilobytes = setting("BIFROST_BENCH_FILE_KB", 16);
    let provider = provider();
    let local = tempfile::tempdir().unwrap();
    make_tree(local.path(), files, kilobytes);
    let run = format!(
        "upload/bench-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let base = make_remote_dir(&provider, &run).await;
    println!(
        "\n{files} files of {kilobytes} KB in {} folders over SFTP (server on this machine)",
        (files / 50).max(1)
    );

    println!("{:<44} {:>10} {:>16}", "UPLOAD", "time", "speed");
    let mut last_upload = None;
    for (label, options) in [
        ("1 file at a time (like Explorer)", tuned(1, 1)),
        ("4 files at once", tuned(4, 4)),
        ("8 files at once", tuned(8, 8)),
        ("16 files at once", tuned(16, 16)),
        ("adaptive 8 to 16 (what the app uses)", tuned(8, 16)),
    ] {
        let destination = make_remote_dir(
            &provider,
            &format!(
                "{run}/up-{}",
                label.replace(' ', "-").replace(['(', ')'], "")
            ),
        )
        .await;
        let took = timed_copy(
            vec![Endpoint::Local(local.path().join("set"))],
            destination.clone(),
            options,
        )
        .await;
        report(label, files, took);
        last_upload = Some(destination);
    }

    println!("\n{:<44} {:>10} {:>16}", "DOWNLOAD", "time", "speed");
    let uploaded = match last_upload.unwrap() {
        Endpoint::Remote { provider, path } => Endpoint::Remote {
            provider,
            path: RemotePath::parse(&format!("{}/set", path.as_str())).unwrap(),
        },
        local => local,
    };
    for (label, options) in [
        ("1 file at a time (like Explorer)", tuned(1, 1)),
        ("4 files at once", tuned(4, 4)),
        ("adaptive 8 to 16 (what the app uses)", tuned(8, 16)),
    ] {
        let target = tempfile::tempdir().unwrap();
        let took = timed_copy(
            vec![uploaded.clone()],
            Endpoint::Local(target.path().to_path_buf()),
            options,
        )
        .await;
        report(label, files, took);
    }

    println!("\n{:<44} {:>10} {:>16}", "DELETE", "time", "speed");
    for (label, options) in [
        ("1 file at a time (like Explorer)", tuned(1, 1)),
        ("4 files at once", tuned(4, 4)),
        ("adaptive 8 to 16 (what the app uses)", tuned(8, 16)),
    ] {
        let doomed = make_remote_dir(
            &provider,
            &format!(
                "{run}/del-{}",
                label.replace(' ', "-").replace(['(', ')'], "")
            ),
        )
        .await;
        timed_copy(
            vec![Endpoint::Local(local.path().join("set"))],
            doomed.clone(),
            tuned(16, 16),
        )
        .await;
        let target = match doomed {
            Endpoint::Remote { provider, path } => Endpoint::Remote {
                provider,
                path: RemotePath::parse(&format!("{}/set", path.as_str())).unwrap(),
            },
            local => local,
        };
        report(label, files, timed_delete(target, options).await);
    }

    // Leave the server as it was found.
    timed_delete(base, tuned(16, 16)).await;
}
