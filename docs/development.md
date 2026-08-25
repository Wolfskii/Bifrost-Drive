# Development

## Prerequisites

Install Rust stable with Cargo, Node.js LTS with npm, Task, and the Tauri 2 prerequisites for your host. Windows development additionally needs Visual Studio Build Tools with the Desktop C++ workload, Windows SDK, and WebView2. Docker Desktop is needed for integration services. Do not install these globally through the repository.

## Commands

Run commands from the repository root:

| Command                | Purpose                                              |
| ---------------------- | ---------------------------------------------------- |
| `task setup`           | Check tools, install dependencies, fetch Rust crates |
| `task dev`             | Start the Tauri desktop app                          |
| `task build`           | Build Rust and frontend                              |
| `task check`           | Compile Rust and build frontend                      |
| `task test`            | Run unit and integration test suites                 |
| `task lint`            | Run Clippy and ESLint                                |
| `task format`          | Format Rust and frontend                             |
| `task format:check`    | Verify formatting                                    |
| `task docker:up`       | Start MinIO integration service                      |
| `task docker:down`     | Stop integration services                            |
| `task db:migrate`      | Apply SQLite migrations                              |
| `task package:windows` | Build Windows x64 bundle on Windows                  |
| `task release:dry-run` | Validate release metadata without publishing         |

The real S3 integration test uses `task test:integration`. If ports 9000 or 9001 are already in use, set `MINIO_API_PORT`, `MINIO_CONSOLE_PORT`, and `BIFROST_S3_ENDPOINT` for that invocation, for example `MINIO_API_PORT=19000 MINIO_CONSOLE_PORT=19001 BIFROST_S3_ENDPOINT=http://127.0.0.1:19000 task test:integration`.

The root `package.json` declares npm as the package manager. `npm ci` is used in CI from the committed `package-lock.json`; use `npm install` when dependency manifests change.

## Workflow

Feature branches merge into `develop`. Stable release proposals merge into protected `main`. Use Conventional Commits. Before a pull request, run `task format`, `task check`, `task lint`, and `task test`.

## Database

Use `BIFROST_DATABASE_URL` for local migration testing. Never point development commands at production data. Use `task db:reset` only for the local `bifrost-drive.db` file.
