CREATE TABLE cache_entries (
    connection_id TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    local_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    last_accessed TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    active_transfer INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (connection_id, remote_path),
    FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE CASCADE
);

CREATE TABLE transfer_queue (
    id TEXT PRIMARY KEY NOT NULL,
    connection_id TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    direction TEXT NOT NULL,
    status TEXT NOT NULL,
    total_bytes INTEGER,
    transferred_bytes INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE CASCADE
);

CREATE INDEX cache_entries_last_accessed_idx ON cache_entries(last_accessed);
CREATE INDEX transfer_queue_status_idx ON transfer_queue(status);
CREATE INDEX transfer_queue_connection_idx ON transfer_queue(connection_id);
