# Transfers

The provider-neutral bounded transfer queue is implemented with durable-schema support. It models queue state, bounds concurrency, supports progress, pause/resume/cancel/retry, uses capped exponential backoff, persists snapshots, restores interrupted jobs as pending, and streams provider I/O into atomic cache commits. Hydration, uploads, and recent activity are exposed by the desktop host. Byte-range resume checkpoints remain future work.

Activity and history must omit credentials, access tokens, and file contents.
