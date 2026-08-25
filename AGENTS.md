# Bifrost Drive Agent Guide

## Purpose

Bifrost Drive is a Rust-powered desktop client that makes remote storage feel local. The first release targets Windows 11 22H2+ and uses the Windows Cloud Files API (CFAPI), while shared providers, cache, transfers, synchronization, metadata, credentials, and API contracts remain platform-independent.

## Read First

Before changing code, read `README.md`, `docs/architecture.md`, `docs/development.md`, and this file. Prefer the existing abstraction and update relevant documentation and tests whenever behavior or architecture changes.

## Architecture

- `crates/bifrost-common`: provider-neutral identifiers, paths, states, capabilities, and errors.
- `crates/bifrost-storage`: provider port and streaming contracts.
- `crates/bifrost-db`: SQLite repositories and versioned migrations.
- `crates/bifrost-crypto`: credential-store ports and secret redaction.
- `crates/bifrost-api`: typed UI/service commands and events.
- `crates/bifrost-core`: application orchestration; it must not depend on concrete providers or OS APIs.
- `crates/bifrost-s3`, `bifrost-sftp`, and `bifrost-webdav`: isolated provider implementations.
- `platforms/windows`, `platforms/macos`, and `platforms/linux`: native filesystem, credential, shell, notification, and installer adapters.
- `apps/desktop`: thin React presentation and Tauri composition layer.
- `apps/cli`: thin local-service client when introduced; it must not duplicate business logic.

Dependency direction points inward: common -> contracts/persistence -> core -> adapters. Shared crates never import Tauri or Windows/macOS/Linux APIs.

## Commands

Use Taskfile as the canonical interface: `task setup`, `task dev`, `task build`, `task test`, `task lint`, `task format`, `task format:check`, `task check`, `task docker:up`, `task docker:down`, `task db:migrate`, `task package:windows`, `task version`, and `task release:dry-run`. Run `task format`, `task check`, and relevant tests before declaring work complete.

## Non-negotiable Rules

- Never put provider-specific logic in `bifrost-core`.
- Never put Windows, macOS, or Linux APIs in shared crates.
- Never store credentials in SQLite plaintext; the database stores references only.
- Never log credentials, access tokens, passwords, private keys, or file contents.
- Never silently overwrite conflicting files.
- Never disable TLS certificate validation or SSH host verification by default.
- Never use blocking I/O inside async runtime code without an explicit blocking boundary.
- Never perform unbounded concurrent transfers.
- Never implement a filesystem driver when the native platform API is appropriate.
- Never claim a provider capability it does not actually support.
- Never break or rewrite existing database migrations.
- Never commit generated secrets, certificates, or private keys.
- Never manually edit generated lockfiles unless the package manager requires it.
- Never add `todo!()` or `unimplemented!()` to production paths to make compilation succeed.
- Never create fake implementations; return a meaningful typed unsupported error and document the boundary.

## Adding a Provider

Add a dedicated crate implementing the `StorageProvider` contract. Map native errors into typed provider errors, declare only verified capabilities, use streaming I/O, add provider contract tests against a real service, and add configuration/API/UI support only after the provider works. Keep credentials in native stores.

## Adding an OS Integration

Add an adapter below `platforms/<os>/`. Define a narrow port in shared contracts, inject the adapter from the application host, and keep OS handles/types out of shared crates. Add native tests where the platform is available and document limitations for other hosts.

## Database Changes

Every schema change gets a new forward-only migration in `crates/bifrost-db/migrations/`. Use transactions, foreign keys, durable state, and migration tests from an empty database and representative prior fixtures. Never store file contents or secret material in SQLite.

## Releases

Feature work flows through `develop`; protected `main` is the stable release source. Conventional Commits feed automated version proposals. The root Cargo workspace version is canonical and must stay synchronized with the frontend package. Release CI must pass checks, build the real Windows artifact, generate checksums and notes from actual changes, and create no tag or release after a failed build. Signing material is provided only through protected CI secrets.

## Unfinished Work

CFAPI, macOS File Provider, Linux FUSE, SFTP, WebDAV, native credential backends, cache, transfers, sync, and release packaging must be marked Planned until implemented and tested. S3 capabilities may be exposed only where the provider implementation and tests verify them. Do not expose unavailable capabilities in the UI.
