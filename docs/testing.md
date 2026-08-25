# Testing

Rust unit tests cover path normalization, capabilities, state transitions, retry decisions, cache eviction, durable transfer recovery, synchronization persistence, conflict detection, metadata, FTP/FTPS endpoint validation, SMB endpoint validation, and FUSE boundaries. Provider contract tests use real services rather than mocking every operation: MinIO for S3, rclone WebDAV, atmoz SFTP with generated known_hosts and ephemeral Ed25519 key files, plus opt-in FTP/FTPS and SMB fixtures through `BIFROST_FTP_INTEGRATION=1` and `BIFROST_SMB_INTEGRATION=1`.

Frontend tests use Vitest and component testing. Windows CFAPI behavior requires native Windows 11 22H2+ tests on NTFS, including Explorer-facing install, hydration, local edits, recovery, and uninstall. CI must clearly distinguish compile checks from native integration tests.
