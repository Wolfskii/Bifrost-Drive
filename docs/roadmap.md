# Roadmap

## Phase 1: Foundation

- Tauri 2 desktop shell
- Rust workspace and typed contracts
- SQLite migration boundary
- Native credential-store abstraction
- CI and developer workflow

## Phase 2: Windows MVP

- [x] S3, SFTP, and WebDAV
- [x] Metadata/file cache and offline pinning primitives
- [x] Durable bounded transfers and conflict-safe one-shot synchronization
- [x] Windows CFAPI registration, placeholders, and callback completion
- [x] Background synchronization scheduler and conflict resolution UI
- [x] CFAPI local close, delete, and rename mutation routing
- [x] Tray entry point and durable activity history
- [x] Start-on-boot setting through the native OS startup mechanism
- [ ] Windows Explorer VM acceptance and crash/reconnect validation
- [x] Native notifications and durable transfer history
- [x] Windows x64 signing and updater workflow configuration
- [x] Linux AppImage and macOS update artifacts in development releases
- [ ] Provision protected release secrets and execute a signed stable release

## Phase 3: Expansion

- [x] SMB and FTP/FTPS provider slices with opt-in contract tests
- [x] macOS Keychain and Linux Secret Service credential adapters
- [x] Read-only Linux FUSE provider mount and CI compile test
- [ ] Nextcloud-specific discovery and authentication
- [x] macOS File Provider Swift target boundary and CI package tests
- [ ] macOS File Provider Rust transport wiring, signing, and Finder acceptance
- [ ] Writable Linux FUSE mutation semantics and native acceptance
- Additional cloud providers, sharing, versions, and locking

## Phase 4: Advanced

- Cryptomator compatibility
- Custom versioning
- Enterprise functionality
- Stable beta/nightly release channels

Items are not supported until their implementation and tests exist.
