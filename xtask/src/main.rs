use anyhow::{bail, Context, Result};
use std::{fs, path::Path};

fn main() -> Result<()> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    match command.as_str() {
        "version-check" => version_check(),
        "release-check" => version_check(),
        "release-dry-run" => {
            version_check()?;
            println!("Release dry run: metadata is valid; CI will decide whether commits require a release.");
            Ok(())
        }
        "db-reset" => {
            let path = Path::new("bifrost-drive.db");
            if path.exists() {
                fs::remove_file(path).context("remove local database")?;
            }
            println!("Removed local database if it existed.");
            Ok(())
        }
        _ => {
            println!("xtask commands: version-check, release-check, release-dry-run, db-reset");
            Ok(())
        }
    }
}

fn version_check() -> Result<()> {
    let cargo: toml::Value =
        toml::from_str(&fs::read_to_string("Cargo.toml").context("read workspace Cargo.toml")?)?;
    let root: serde_json::Value = serde_json::from_str(
        &fs::read_to_string("package.json").context("read root package.json")?,
    )?;
    let desktop: serde_json::Value = serde_json::from_str(
        &fs::read_to_string("apps/desktop/package.json").context("read desktop package.json")?,
    )?;
    let root_version = root["version"].as_str().context("root package version")?;
    let desktop_version = desktop["version"]
        .as_str()
        .context("desktop package version")?;
    let cargo_version = cargo["workspace"]["package"]["version"]
        .as_str()
        .context("workspace package version")?;
    if root_version != desktop_version || root_version != cargo_version {
        bail!(
            "version mismatch: cargo {cargo_version}, root {root_version}, desktop {desktop_version}"
        );
    }
    println!("Version {root_version} is synchronized.");
    Ok(())
}
