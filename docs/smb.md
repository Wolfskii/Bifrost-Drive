# SMB

The SMB provider uses the pure-Rust SMB2/3 client and supports authenticated `smb://host/share` connections, directory listing, metadata, range reads, streamed writes, directory creation, rename, and deletion. SMB reconnects are bounded by the client library and do not replay mutations after an ambiguous network failure.

SMB signing and encryption are negotiated by the protocol client. Guest access is not exposed by the desktop wizard. The provider is available through the desktop connection wizard.
