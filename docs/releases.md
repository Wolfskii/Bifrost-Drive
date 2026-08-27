# Releases

Development releases currently run from `develop` in a single dependency-ordered workflow. Parallel Rust and frontend gates run first, one version-planning job selects at least `0.2.0` and then the next patch after the latest release unless a dispatch input overrides it, and separate Windows/Linux/macOS jobs produce signed packages. The publishing job reuses those artifacts, publishes checksums and `latest.json`, and creates no tag or release after a failed check or build. Production Authenticode certificates and stable releases from protected `main` remain future release-process work. See [release.md](release.md) for secret provisioning.

Release CI runs checks, builds Windows/Linux/macOS artifacts, generates checksums and release notes from actual changes, then creates a normal GitHub Release from `develop`. A failed build must create neither tag nor release. Development Authenticode signing is self-signed; only the Tauri updater key is required as a GitHub secret.

The required Windows artifact name is `Bifrost-Drive-Setup-x64.exe`; Linux AppImage, RPM, and macOS artifacts are published alongside it. AppImage is the signed Linux updater artifact, while RPM is an additional installation format. The self-signed development installer is for testing and is not trusted by Windows by default.
