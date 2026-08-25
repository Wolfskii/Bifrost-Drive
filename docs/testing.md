# Testing

Rust unit tests cover path normalization, capabilities, state transitions, retry decisions, cache eviction, transfer management, conflict detection, and metadata. Provider contract tests use pinned real Docker services rather than mocking every operation: MinIO for S3, rclone WebDAV, and atmoz SFTP with a generated known_hosts file.

Frontend tests use Vitest and component testing. Windows CFAPI behavior requires native Windows 11 22H2+ tests on NTFS, including Explorer-facing install, hydration, local edits, recovery, and uninstall. CI must clearly distinguish compile checks from native integration tests.
