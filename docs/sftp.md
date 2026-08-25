# SFTP

The SFTP adapter supports password and private-key authentication with verified host keys. It supports configurable ports, known_hosts verification, streaming file reads/writes, listing, metadata, rename, and deletion. The desktop wizard uses the current user's standard `.ssh/known_hosts` file automatically and does not require entering its path. For new servers, the optional trust-on-first-use setting records the first key in that file and continues to reject changed keys; strict verification remains the default. The pinned atmoz integration fixture exercises password authentication and an ephemeral Ed25519 private key. Private keys and passphrases are stored in the native credential store; the desktop wizard accepts a key path and optional passphrase. SSH agents, keyboard-interactive authentication, and OpenSSH configuration remain Planned.

Host verification must never be disabled by default.
