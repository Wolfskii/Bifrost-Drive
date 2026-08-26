# Development

## Prerequisites

Install Rust stable with Cargo, Node.js LTS with npm, Task, and the Tauri 2 prerequisites for your host. Windows development additionally needs Visual Studio Build Tools with the Desktop C++ workload, Windows SDK, WebView2, and WinFsp 2.1 or later. Install WinFsp from an elevated terminal with `choco install winfsp -y`; end-user packages bundle this prerequisite automatically. Docker Desktop is needed for integration services. Release Windows installers use per-machine installation under `Program Files` and require administrator approval.

## Commands

Run commands from the repository root:

| Command                       | Purpose                                              |
| ----------------------------- | ---------------------------------------------------- |
| `task setup`                  | Check tools, install dependencies, fetch Rust crates |
| `task dev`                    | Start the Tauri desktop app                          |
| `task build`                  | Build Rust and frontend                              |
| `task check`                  | Compile Rust and build frontend                      |
| `task test`                   | Run unit and integration test suites                 |
| `task lint`                   | Run Clippy and ESLint                                |
| `task format`                 | Format Rust and frontend                             |
| `task format:check`           | Verify formatting                                    |
| `task docker:up`              | Start MinIO, WebDAV, and SFTP services               |
| `task docker:down`            | Stop provider integration services                   |
| `task db:migrate`             | Apply SQLite migrations                              |
| `task package:windows`        | Build Windows x64 bundle on Windows                  |
| `task cleanup:windows-drives` | Select stale Bifrost Explorer drive entries          |
| `task release:dry-run`        | Validate release metadata without publishing         |

The real S3, WebDAV, and SFTP integration tests use `task test:integration`. If the default ports are already in use, set `MINIO_API_PORT`, `MINIO_CONSOLE_PORT`, `WEBDAV_PORT`, `SFTP_PORT`, `BIFROST_S3_ENDPOINT`, and `BIFROST_WEBDAV_ENDPOINT`, for example `MINIO_API_PORT=19000 MINIO_CONSOLE_PORT=19001 WEBDAV_PORT=18080 SFTP_PORT=2222 BIFROST_S3_ENDPOINT=http://127.0.0.1:19000 BIFROST_WEBDAV_ENDPOINT=http://127.0.0.1:18080/ task test:integration`.

The root `package.json` declares npm as the package manager. `npm ci` is used in CI from the committed `package-lock.json`; use `npm install` when dependency manifests change.

On Windows, `task cleanup:windows-drives` opens a terminal menu containing only WinFsp identities created by Bifrost whose recorded owner process is no longer active. Cleanup removes the selected Bifrost network mapping, its letter-only and Bifrost MountPoints2 metadata, and its drive icon override, then restarts Explorer. Generic Explorer drive history is never listed. The Windows system drive is hard-blocked at discovery and deletion time, and every current non-Bifrost logical drive or unrelated network mapping is rejected.

## Workflow

Feature branches merge into `develop`. Stable release proposals merge into protected `main`. Use Conventional Commits. Before a pull request, run `task format`, `task check`, `task lint`, and `task test`.

One GitHub Actions workflow handles pushes to `develop`: frontend and Rust checks gate three parallel operating-system package builds, and the release job publishes only the artifacts produced by those successful builds. Manual runs may provide an explicit release version; otherwise CI increments the latest published release patch version.

## Database

Use `BIFROST_DATABASE_URL` for local migration testing. Never point development commands at production data. Use `task db:reset` only for the local `bifrost-drive.db` file.
