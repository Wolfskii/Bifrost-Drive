CREATE TABLE sync_entries (
    connection_id TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    state TEXT NOT NULL,
    base_fingerprint TEXT,
    local_fingerprint TEXT,
    remote_fingerprint TEXT,
    last_error TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (connection_id, remote_path),
    FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE CASCADE
);

CREATE TABLE conflicts (
    id TEXT PRIMARY KEY NOT NULL,
    connection_id TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    local_fingerprint TEXT,
    remote_fingerprint TEXT,
    detected_at TEXT NOT NULL,
    resolution TEXT,
    resolved_at TEXT,
    FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE CASCADE
);

CREATE INDEX sync_entries_state_idx ON sync_entries(state);
CREATE INDEX conflicts_connection_idx ON conflicts(connection_id);