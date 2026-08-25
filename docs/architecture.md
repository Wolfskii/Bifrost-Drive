# Architecture

Bifrost Drive separates provider-neutral application services from native operating-system integrations.

```mermaid
flowchart TD
  UI[React and Tauri UI] --> API[Typed API]
  CLI[CLI Planned] --> API
  API --> CORE[bifrost-core]
  CORE --> DB[bifrost-db]
  CORE --> CACHE[bifrost-cache]
  CORE --> TRANSFER[bifrost-transfer]
  CORE --> SYNC[bifrost-sync reconciliation primitives]
  PROVIDERS[Provider crates] --> STORAGE[bifrost-storage contracts]
  CORE --> STORAGE
  PLATFORM[platforms/windows, macos, linux] --> API
```

The shared crates do not import Tauri or operating-system APIs. Provider implementations are isolated in their own crates and expose only verified capabilities. The desktop host persists transfers, hydrates all current providers, and runs one-shot synchronization; background scheduling and native acceptance coverage remain separate concerns.

## Database

SQLite stores connections, metadata, cache state, transfers, activity, history, pins, and conflicts. It never stores file contents or secret material. Every schema change is a new forward-only migration under `crates/bifrost-db/migrations/`. Migrations run transactionally and are tested from an empty database.

## Native boundaries

Windows CFAPI, Credential Manager, Explorer services, and installer integration belong under `platforms/windows/`. macOS File Provider/Keychain and Linux FUSE/Secret Service are Planned adapters. The CFAPI adapter provides registration, callback dispatch, provider hydration, placeholder transfer, and explicit completion through `CfExecute`; Windows VM acceptance remains required. The design uses placeholders and hydration, not a legacy kernel filesystem driver or periodic mirror folder.
