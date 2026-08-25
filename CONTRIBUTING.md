# Contributing

Bifrost Drive is developed in small, reviewable vertical slices.

## Setup

Install Rust, Node.js LTS with npm, Task, Docker Desktop, and the Tauri prerequisites for your host. Run `task setup`, then `task check`.

## Workflow

Feature branches use `feature/<short-name>`, fixes use `fix/<short-name>`, and documentation changes use `docs/<short-name>`. Changes merge into `develop`; stable releases are promoted to protected `main`.

Use Conventional Commits such as `feat: add s3 listing` or `fix: preserve transfer checkpoint`.

## Before Opening a Pull Request

Run `task format`, `task check`, `task lint`, and `task test`. Add or update focused tests and documentation. Provider changes require real integration coverage where practical. Platform changes require native validation or an explicit documented limitation.

Do not commit secrets, certificates, generated artifacts, or lockfile edits made outside the package manager.

## Architecture Changes

Read `AGENTS.md` and `docs/architecture.md`. Preserve the shared-core/native-adapter boundary. New providers get their own crate; new OS functionality belongs under `platforms/`.

## Releases

Release automation runs from `main` after the version proposal is reviewed. Do not create release tags manually during normal development. See [docs/releases.md](docs/releases.md).
