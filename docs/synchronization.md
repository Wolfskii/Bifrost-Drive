# Synchronization

Synchronization persists remote, local, and base revisions and will not overwrite when both sides changed since the last common revision. The desktop host schedules persisted entries, routes one-sided changes through bounded transfers, and exposes durable keep-local and keep-remote conflict resolution. Keep-both and rename-conflict materialization remain unsupported until conflict filenames can be created without overwriting data.

The target state model includes `ONLINE`, `OFFLINE`, `SYNCING`, `UPLOADING`, `DOWNLOADING`, `UP_TO_DATE`, `ERROR`, `CONFLICT`, `IGNORED`, `PINNED`, and `PLACEHOLDER`. Network recovery, sleep/wake, retries, credential expiry, and process restart are first-class cases.
