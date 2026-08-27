# Filesystem Semantics

The filesystem layer normalizes remote paths before reaching native adapters. Windows uses writable WinFsp drive mounts and CFAPI sync roots. Native Linux packages expose read-only FUSE mounts at `<mount parent>/<connection name>` and default the parent to `$XDG_RUNTIME_DIR/bifrost-drive`, outside Home; writable Linux mutation semantics remain planned. Bifrost refuses to hide a non-empty host directory with a mount. It must handle traversal, separators, Unicode, case collisions, reserved Windows names, path length, symlinks, timestamps, locks, sparse files, and concurrent access without silently changing user data.

Windows will use CFAPI placeholders rather than a traditional filesystem driver. A placeholder represents remote metadata; opening it requests hydration into the local cache. Pinning maps to a full, offline-available file. Unsupported provider semantics must be visible as limitations.
