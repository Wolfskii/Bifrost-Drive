# Bifrost Drive

**One gateway. Every storage.**

Bifrost Drive is a cross-platform desktop client for remote storage. It is designed to make S3, SFTP, WebDAV, and future providers feel native while keeping storage, caching, transfers, synchronization, and credentials in a shared Rust core.

## Status

Early foundation. The repository currently contains shared Rust contracts, SQLite migrations, native Windows/macOS/Linux credential adapters, connection wizards for S3/SFTP/WebDAV/FTP/SMB, provider implementations for S3/SFTP/WebDAV/FTP/SMB, remote root browsing, durable cache-backed transfers, scheduled conflict-safe synchronization, CFAPI hydration and local mutation routing, tray support, native notifications, signed updater configuration, durable activity history, and real provider tests. Native Explorer/File Provider acceptance coverage and writable Linux FUSE semantics remain in progress.

## Supported Targets

- Windows 11 22H2+ is the first-release target. The CFAPI adapter registers roots, creates placeholders, resolves provider data, and completes Explorer fetch callbacks; Windows VM acceptance coverage is still required.
- macOS Keychain and File Provider package support are available; signed extension transport wiring remains platform-specific.
- Linux Secret Service and read-only FUSE integration are available; writable FUSE mutation semantics remain in progress.

## Development

Prerequisites: Rust stable with Cargo, Node.js LTS with npm, Task, and Tauri 2 host prerequisites. Docker Desktop is required for provider integration services.

```text
task setup
task dev
task format
task check
task test
```

See [docs/development.md](docs/development.md) for the full command reference and platform prerequisites.

## Architecture

Read [AGENTS.md](AGENTS.md) and [docs/architecture.md](docs/architecture.md) before changing shared boundaries. The UI is thin. Providers are isolated. Native filesystem APIs live only under `platforms/`.

## Security

Credentials are intended for native OS credential stores. SQLite stores references and non-secret configuration only. TLS and SSH verification remain enabled by default. See [SECURITY.md](SECURITY.md) and [docs/security.md](docs/security.md).

## Roadmap

See [docs/roadmap.md](docs/roadmap.md). Current work prioritizes S3, then SFTP and WebDAV, followed by cache, bounded transfers, conflict-safe synchronization, and Windows CFAPI.

## License

Apache-2.0. See [LICENSE](LICENSE).
