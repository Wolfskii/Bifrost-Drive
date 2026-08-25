# Transfers

The bounded transfer engine is **Planned**. It will persist queue entries and checkpoints, stream large files, bound concurrency globally and per connection, support progress, pause/resume/cancel/retry, and use capped exponential backoff. Resumable behavior is enabled only for providers with the relevant capability.

Activity and history must omit credentials, access tokens, and file contents.
