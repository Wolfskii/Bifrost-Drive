-- A log of Explorer-initiated copies, moves and deletes: one row per batch, written when it
-- starts and updated when it ends. Holds paths and counts only, never file contents or secrets.
CREATE TABLE transfer_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    mode TEXT NOT NULL,
    direction TEXT NOT NULL,
    connection TEXT NOT NULL,
    destination TEXT NOT NULL,
    items_json TEXT NOT NULL DEFAULT '[]',
    item_count INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    total_files INTEGER NOT NULL DEFAULT 0,
    done_files INTEGER NOT NULL DEFAULT 0,
    skipped_files INTEGER NOT NULL DEFAULT 0,
    failed_files INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER NOT NULL DEFAULT 0,
    done_bytes INTEGER NOT NULL DEFAULT 0,
    failures_json TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX transfer_history_started_at_idx ON transfer_history(started_at);
