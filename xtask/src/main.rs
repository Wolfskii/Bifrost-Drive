use anyhow::{bail, Context, Result};
use semver::Version;
use std::{fs, path::Path, process::Command};

fn main() -> Result<()> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    match command.as_str() {
        "version-check" => version_check(),
        "version-bump" => version_bump(),
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
            println!("xtask commands: version-check, version-bump, release-check, release-dry-run, db-reset");
            Ok(())
        }
    }
}

fn version_bump() -> Result<()> {
    let current = synchronized_version()?;
    if version_changed_in_commit()? {
        version_check()?;
        println!("Manual version {current} retained.");
        return Ok(());
    }

    let mut next = Version::parse(&current).context("parse synchronized version")?;
    next.patch += 1;
    let next = next.to_string();
    replace_workspace_version(&current, &next)?;
    replace_json_version("package.json", &current, &next)?;
    replace_json_version("apps/desktop/package.json", &current, &next)?;
    println!("Bumped version from {current} to {next}.");
    Ok(())
}

fn synchronized_version() -> Result<String> {
    let cargo: toml::Value =
        toml::from_str(&fs::read_to_string("Cargo.toml").context("read workspace Cargo.toml")?)?;
    let version = cargo["workspace"]["package"]["version"]
        .as_str()
        .context("workspace package version")?;
    Ok(version.to_owned())
}

fn version_changed_in_commit() -> Result<bool> {
    let output = Command::new("git")
        .args([
            "diff",
            "--unified=0",
            "HEAD^",
            "HEAD",
            "--",
            "Cargo.toml",
            "package.json",
            "apps/desktop/package.json",
        ])
        .output()
        .context("inspect previous commit")?;
    if !output.status.success() {
        return Ok(false);
    }
    let diff = String::from_utf8_lossy(&output.stdout);
    Ok(diff.lines().any(|line| {
        line.starts_with('+')
            && !line.starts_with("+++")
            && (line.contains("version = \"") || line.contains("\"version\": \""))
    }))
}

fn replace_workspace_version(current: &str, next: &str) -> Result<()> {
    let path = Path::new("Cargo.toml");
    let content = fs::read_to_string(path)?;
    let marker = "[workspace.package]";
    let start = content.find(marker).context("workspace package section")?;
    let suffix = &content[start..];
    let old = format!("version = \"{current}\"");
    let offset = suffix
        .find(&old)
        .context("workspace package version line")?;
    let absolute = start + offset;
    let mut updated = content;
    updated.replace_range(
        absolute..absolute + old.len(),
        &format!("version = \"{next}\""),
    );
    fs::write(path, updated)?;
    Ok(())
}

fn replace_json_version(path: &str, current: &str, next: &str) -> Result<()> {
    let path_ref = Path::new(path);
    let content = fs::read_to_string(path_ref)?;
    let old = format!("\"version\": \"{current}\"");
    let new = format!("\"version\": \"{next}\"");
    if !content.contains(&old) {
        bail!("version line not found in {path}");
    }
    fs::write(path_ref, content.replacen(&old, &new, 1))?;
    Ok(())
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
