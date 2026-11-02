//! `cargo run -p bifrost-windows-storage --example classify -- C:\ D:\` prints each path's class.
fn main() {
    for path in std::env::args().skip(1) {
        println!(
            "{path}: {:?}",
            bifrost_windows_storage::classify(std::path::Path::new(&path))
        );
    }
}
