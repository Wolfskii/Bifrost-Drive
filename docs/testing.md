# Testing

Rust unit tests cover path normalization, capabilities, state transitions, retry decisions, cache eviction, durable transfer recovery, synchronization persistence, conflict detection, metadata, FTP/FTPS endpoint validation, SMB endpoint validation, and FUSE boundaries. `task test:connections` exercises provider contracts against real local services: MinIO for S3, Apache for WebDAV, atmoz SFTP with generated known_hosts and ephemeral Ed25519 key files, vsftpd for FTP, and Samba for SMB. The task removes all fixture containers and volumes after success or failure.

Frontend tests use Vitest and component testing. Windows CFAPI behavior requires native Windows 11 22H2+ tests on NTFS, including Explorer-facing install, hydration, local edits, recovery, and uninstall. CI must clearly distinguish compile checks from native integration tests.
