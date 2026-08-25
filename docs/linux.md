# Linux

Linux support includes a read-only FUSE adapter backed by the shared `StorageProvider` contract and a Secret Service credential adapter using keyring. Install `libfuse3-dev` and ensure `/dev/fuse` is available before mounting. The FUSE mount deliberately advertises read-only permissions until local mutation semantics and Linux-native acceptance tests are complete. Mount permissions, file watching, cache paths, and disconnect behavior remain Linux-native acceptance concerns.

No Linux filesystem mount support is claimed by the current build.
