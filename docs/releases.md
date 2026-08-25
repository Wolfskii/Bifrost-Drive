# Releases

Stable releases run from protected `main`. Feature work lands in `develop`. The release workflow validates Rust and frontend gates, signs the Windows installer and Tauri updater artifacts when protected secrets are present, publishes checksums and `latest.json`, and creates no release after a failed check. Conventional Commits and automated version proposals remain future release-process work; the root Cargo workspace version is canonical and must remain synchronized with the frontend package. See [release.md](release.md) for secret provisioning.

Release CI will run checks, build and test the real Windows x64 artifact, generate checksums and release notes from actual changes, then create the tag and GitHub Release. A failed build must create neither tag nor release. Signing uses protected GitHub environment secrets and is optional for unsigned development builds.

The required artifact name is `Bifrost-Drive-Setup-x64.exe`. macOS and Linux artifacts remain unclaimed until their native integrations are implemented and tested.
