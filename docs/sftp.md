# SFTP

The SFTP adapter is implemented for password authentication and verified host keys. It supports configurable ports, known_hosts verification, streaming file reads/writes, listing, metadata, rename, and deletion. The pinned atmoz integration fixture exercises this contract with a generated host-key file, and the desktop wizard can create and test SFTP connections. Public-key authentication, encrypted keys/passphrases, SSH agents, keyboard-interactive authentication, and OpenSSH configuration remain Planned.

Host verification must never be disabled by default.
