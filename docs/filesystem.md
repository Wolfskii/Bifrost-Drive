# Filesystem Semantics

The filesystem layer is **Planned**. Remote paths will pass through a normalization boundary before reaching a native adapter. It must handle traversal, separators, Unicode, case collisions, reserved Windows names, path length, symlinks, timestamps, locks, sparse files, and concurrent access without silently changing user data.

Windows will use CFAPI placeholders rather than a traditional filesystem driver. A placeholder represents remote metadata; opening it requests hydration into the local cache. Pinning maps to a full, offline-available file. Unsupported provider semantics must be visible as limitations.
