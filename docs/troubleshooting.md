# Troubleshooting

- `cargo check` fails on Windows: install Visual Studio Build Tools with the C++ workload and Windows SDK.
- Tauri cannot start: install WebView2 and verify the Tauri host prerequisites.
- MinIO tests cannot connect: run `task docker:up` and wait for the health check.
- Migration errors: confirm `BIFROST_DATABASE_URL` points to a writable SQLite database and inspect the first failing migration; never edit an applied migration.
- CFAPI behavior is unavailable on the current host: run the native test suite on a Windows 11 22H2+ NTFS VM.

Never work around a security error by disabling TLS or host verification.
