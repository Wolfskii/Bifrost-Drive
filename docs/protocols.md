# Protocols

Provider implementations are isolated behind `bifrost-storage::StorageProvider`. Each provider returns a capability set and maps native failures into typed errors. The MVP order is S3, SFTP, and WebDAV. Nextcloud is a dedicated UI connection kind backed by WebDAV.

Providers must stream large reads and writes, paginate listings, preserve TLS/SSH verification, and avoid advertising unsupported features. SMB, FTP/FTPS, and additional cloud services are Planned.
