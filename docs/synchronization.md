# Synchronization

Synchronization is **Planned**. The engine will persist remote, local, and base revisions and will not overwrite when both sides changed since the last common revision. Conflicts become durable records and require an explicit keep-local, keep-remote, keep-both, or rename-conflict action.

The target state model includes `ONLINE`, `OFFLINE`, `SYNCING`, `UPLOADING`, `DOWNLOADING`, `UP_TO_DATE`, `ERROR`, `CONFLICT`, `IGNORED`, `PINNED`, and `PLACEHOLDER`. Network recovery, sleep/wake, retries, credential expiry, and process restart are first-class cases.
