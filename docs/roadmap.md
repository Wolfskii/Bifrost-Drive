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
- [ ] Windows Explorer VM acceptance and crash/reconnect validation
- [ ] Native notifications and richer transfer history
- [ ] Windows x64 signing, updater, and automated stable releases

## Phase 3: Expansion

- Nextcloud, SMB, FTP/FTPS
- macOS File Provider
- Linux FUSE
- Additional cloud providers, sharing, versions, and locking

## Phase 4: Advanced

- Cryptomator compatibility
- Custom versioning
- Enterprise functionality
- Stable beta/nightly release channels

Items are not supported until their implementation and tests exist.
