//! Throughput of the batch engine between two local folders, against copying one file at a time
//! (which is how Explorer works). Ignored by default because it writes a lot of data; run it with
//!
//! ```text
//! cargo test -p bifrost-transfer --release --test benchmark -- --ignored --nocapture
//! ```
//!
//! `BIFROST_BENCH_FILES` (default 5000) and `BIFROST_BENCH_FILE_KB` (default 16) set the
//! workload; `BIFROST_BENCH_DIR` chooses the drive the source and destination folders are made
//! on (default: the system temp folder).

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bifrost_transfer::batch::{
    start, start_delete, BatchOptions, BatchRequest, BatchState, DeleteRequest, Endpoint,
    TransferMode,
};

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn make_tree(root: &Path, files: usize, kilobytes: usize) {
    let content = vec![0x5Au8; kilobytes * 1024];
    let folders = (files / 50).max(1);
    for folder in 0..folders {
        std::fs::create_dir_all(root.join(format!("set/folder{folder:04}"))).unwrap();
    }
    for number in 0..files {
        let path = root.join(format!(
            "set/folder{:04}/file{number:06}.dat",
            number % folders
        ));
        std::fs::write(path, &content).unwrap();
    }
}

/// One file after another with the standard library: what Explorer effectively does.
fn serial_copy(from: &Path, to: &Path) -> Duration {
    fn walk(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let started = Instant::now();
    walk(from, to);
    started.elapsed()
}

async fn engine_copy(from: &Path, to: &Path, options: BatchOptions) -> Duration {
    std::fs::create_dir_all(to).unwrap();
    let started = Instant::now();
    let finished = start(
        BatchRequest {
            sources: vec![Endpoint::Local(from.join("set"))],
            destination: Endpoint::Local(to.to_path_buf()),
            mode: TransferMode::Copy,
        },
        options,
    )
    .wait()
    .await;
    assert_eq!(finished.state, BatchState::Completed, "{finished:?}");
    started.elapsed()
}

async fn engine_delete(target: &Path, options: BatchOptions) -> Duration {
    let started = Instant::now();
    let finished = start_delete(
        DeleteRequest {
            targets: vec![Endpoint::Local(target.to_path_buf())],
        },
        options,
    )
    .wait()
    .await;
    assert_eq!(finished.state, BatchState::Completed, "{finished:?}");
    started.elapsed()
}

fn report(label: &str, files: usize, took: Duration) {
    println!(
        "{label:<44} {:>8.2} s {:>10.0} files/s",
        took.as_secs_f64(),
        files as f64 / took.as_secs_f64()
    );
}

fn tuned(start: usize, ceiling: usize) -> BatchOptions {
    BatchOptions {
        max_concurrent: start,
        adaptive_ceiling: ceiling,
        ..BatchOptions::default()
    }
}

fn workspace(prefix: &str) -> tempfile::TempDir {
    let base = std::env::var_os("BIFROST_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(base)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "writes a lot of data; run on demand"]
async fn local_copy_and_delete() {
    let files = setting("BIFROST_BENCH_FILES", 5000);
    let kilobytes = setting("BIFROST_BENCH_FILE_KB", 16);
    let source = workspace("bifrost-bench-source-");
    println!(
        "\n{files} files of {kilobytes} KB in {} folders, copied between folders on the same disk",
        (files / 50).max(1)
    );
    make_tree(source.path(), files, kilobytes);

    println!("{:<44} {:>10} {:>16}", "COPY", "time", "speed");
    let destination = workspace("bifrost-bench-serial-");
    report(
        "one file at a time (like Explorer)",
        files,
        serial_copy(&source.path().join("set"), &destination.path().join("set")),
    );
    drop(destination);

    for (label, options) in [
        ("engine, 1 file at a time", tuned(1, 1)),
        ("engine, 4 files at once", tuned(4, 4)),
        ("engine, 8 files at once", tuned(8, 8)),
        ("engine, 16 files at once", tuned(16, 16)),
        ("engine, adaptive 4 to 16", tuned(4, 16)),
    ] {
        let destination = workspace("bifrost-bench-engine-");
        report(
            label,
            files,
            engine_copy(source.path(), destination.path(), options).await,
        );
    }

    println!("\n{:<44} {:>10} {:>16}", "DELETE", "time", "speed");
    for (label, options) in [
        ("engine, 1 file at a time", tuned(1, 1)),
        ("engine, adaptive 4 to 16", tuned(4, 16)),
    ] {
        let victim = workspace("bifrost-bench-delete-");
        let copy = victim.path().join("set");
        serial_copy(&source.path().join("set"), &copy);
        report(label, files, engine_delete(&copy, options).await);
    }
}
