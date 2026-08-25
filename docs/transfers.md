# Transfers

The provider-neutral bounded transfer queue is implemented with durable-schema support. It models queue state, bounds concurrency, supports progress, pause/resume/cancel/retry, and uses capped exponential backoff. Provider I/O, checkpoints, restart reconciliation, and UI activity remain Planned. Resumable behavior will be enabled only for providers with the relevant capability.

Activity and history must omit credentials, access tokens, and file contents.
