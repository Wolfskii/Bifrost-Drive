# Protocols

Provider implementations are isolated behind `bifrost-storage::StorageProvider`. Each provider returns a capability set and maps native failures into typed errors. Providers that can observe remote changes implement `poll_changes`, which returns changed paths and a resume cursor; others return a typed unsupported error and keep short cache lifetimes. Google Drive implements it with the Changes API. The MVP order is S3, SFTP, and WebDAV. Nextcloud is a dedicated UI connection kind backed by WebDAV.

Providers must stream large reads and writes, paginate listings where the protocol supports it, preserve TLS/SSH verification, and avoid advertising unsupported features. S3, SFTP, WebDAV, FTP/FTPS, SMB, and read-only Immich are implemented provider slices. FTP directory listings are complete protocol responses rather than cursor-paginated results. Immich exposes photos and albums for browsing and original-asset reads; file mutations are unsupported.
