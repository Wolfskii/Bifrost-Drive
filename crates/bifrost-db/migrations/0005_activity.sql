CREATE TABLE activity_events (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    remote_path TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX activity_events_created_at_idx ON activity_events(created_at);
