# Releases

Development releases currently run from `develop`. The workflow synchronizes the patch version, validates Rust and frontend gates, creates an ephemeral self-signed Windows certificate, signs the development installer, signs updater artifacts for Windows/Linux/macOS with the protected Tauri key, publishes checksums and `latest.json`, and creates no release after a failed check. Production Authenticode certificates and stable releases from protected `main` remain future release-process work. See [release.md](release.md) for secret provisioning.

Release CI runs checks, builds Windows/Linux/macOS artifacts, generates checksums and release notes from actual changes, then creates a normal GitHub Release from `develop`. A failed build must create neither tag nor release. Development Authenticode signing is self-signed; only the Tauri updater key is required as a GitHub secret.

The required Windows artifact name is `Bifrost-Drive-Setup-x64.exe`; Linux AppImage and macOS update artifacts are published alongside it. The self-signed development installer is for testing and is not trusted by Windows by default.
