CREATE TABLE connections (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    provider_kind TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    credential_ref TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE metadata_entries (
    connection_id TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    is_directory INTEGER NOT NULL,
    size_bytes INTEGER,
    etag TEXT,
    modified_at TEXT,
    PRIMARY KEY (connection_id, remote_path),
    FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE CASCADE
);

CREATE INDEX metadata_entries_connection_idx ON metadata_entries(connection_id);
