# WebDAV

The WebDAV adapter is implemented with HTTPS/TLS verification by default, Basic Auth, PROPFIND metadata listing, streamed GET/PUT, DELETE, MOVE, and range reads. The pinned rclone integration fixture verifies connection testing, listing, upload, rename, range download, and deletion, and the desktop wizard can create and test WebDAV connections. MKCOL, COPY, and locking capability probes require additional server-specific validation before being exposed as supported.
