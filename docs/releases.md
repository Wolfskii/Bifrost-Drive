# Releases

Stable releases are planned from protected `main`. Feature work lands in `develop`. Conventional Commits feed an automated version proposal; the root Cargo workspace version is canonical and must remain synchronized with the frontend package.

Release CI will run checks, build and test the real Windows x64 artifact, generate checksums and release notes from actual changes, then create the tag and GitHub Release. A failed build must create neither tag nor release. Signing uses protected GitHub environment secrets and is optional for unsigned development builds.

The required artifact name is `Bifrost-Drive-Setup-x64.exe`. macOS and Linux artifacts remain unclaimed until their native integrations are implemented and tested.
