use bifrost_common::{ConnectionId, ProviderKind};
use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database connection failed: {0}")]
    Connect(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("invalid database value: {0}")]
    InvalidValue(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRecord {
    pub id: ConnectionId,
    pub name: String,
    pub kind: ProviderKind,
    pub endpoint: String,
    pub credential_ref: String,
    pub configuration_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncEntryRecord {
    pub connection_id: ConnectionId,
    pub remote_path: String,
    pub state: String,
    pub base_fingerprint: Option<String>,
    pub local_fingerprint: Option<String>,
    pub remote_fingerprint: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRecord {
    pub id: Uuid,
    pub connection_id: ConnectionId,
    pub remote_path: String,
    pub local_fingerprint: Option<String>,
    pub remote_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSummary {
    pub id: Uuid,
    pub connection_id: ConnectionId,
    pub remote_path: String,
    pub local_fingerprint: Option<String>,
    pub remote_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRecord {
    pub id: Uuid,
    pub kind: String,
    pub remote_path: Option<String>,
    pub status: String,
    pub created_at: String,
}

/// What is known when a batch transfer starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTransferHistory {
    pub mode: String,
    pub direction: String,
    pub connection: String,
    pub destination: String,
    /// The paths the user selected, as shown to them (capped by the caller).
    pub items: Vec<String>,
    /// How many items were selected, which can exceed `items.len()`.
    pub item_count: i64,
}

/// How a batch ended (or where it stands), written when it finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferHistoryUpdate {
    pub state: String,
    pub total_files: i64,
    pub done_files: i64,
    pub skipped_files: i64,
    pub failed_files: i64,
    pub total_bytes: i64,
    pub done_bytes: i64,
    /// A JSON array of what went wrong, produced by the caller.
    pub failures_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferHistoryRecord {
    pub id: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub mode: String,
    pub direction: String,
    pub connection: String,
    pub destination: String,
    pub items: Vec<String>,
    pub item_count: i64,
    pub state: String,
    pub total_files: i64,
    pub done_files: i64,
    pub skipped_files: i64,
    pub failed_files: i64,
    pub total_bytes: i64,
    pub done_bytes: i64,
    pub failures_json: String,
}

/// Older rows are dropped when the log grows past this many.
const TRANSFER_HISTORY_LIMIT: i64 = 500;

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn connect(database_url: &str) -> Result<Self, DatabaseError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    pub async fn connect_file(path: &Path) -> Result<Self, DatabaseError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), DatabaseError> {
        match MIGRATOR.run(&self.pool).await {
            Ok(()) => Ok(()),
            Err(sqlx::migrate::MigrateError::VersionMismatch(_)) => {
                self.repair_applied_migration_checksums().await?;
                MIGRATOR.run(&self.pool).await.map_err(DatabaseError::from)
            }
            Err(error) => Err(DatabaseError::from(error)),
        }
    }

    async fn repair_applied_migration_checksums(&self) -> Result<(), DatabaseError> {
        let applied =
            sqlx::query("SELECT version, checksum FROM _sqlx_migrations WHERE success = 1")
                .fetch_all(&self.pool)
                .await?;

        for migration in MIGRATOR.iter() {
            let Some(row) = applied
                .iter()
                .find(|row| row.get::<i64, _>("version") == migration.version)
            else {
                continue;
            };
            if row.get::<Vec<u8>, _>("checksum") != migration.checksum.as_ref() {
                sqlx::query(
                    "UPDATE _sqlx_migrations SET checksum = ? WHERE version = ? AND success = 1",
                )
                .bind(migration.checksum.as_ref())
                .bind(migration.version)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn list_connections(&self) -> Result<Vec<ConnectionRecord>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, name, provider_kind, endpoint, credential_ref, configuration_json FROM connections ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(Self::connection_from_row).collect()
    }

    pub async fn find_connection(
        &self,
        id: ConnectionId,
    ) -> Result<Option<ConnectionRecord>, DatabaseError> {
        let row = sqlx::query(
            "SELECT id, name, provider_kind, endpoint, credential_ref, configuration_json FROM connections WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(Self::connection_from_row).transpose()
    }

    pub async fn insert_connection(
        &self,
        connection: &ConnectionRecord,
    ) -> Result<(), DatabaseError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO connections (id, name, provider_kind, endpoint, credential_ref, configuration_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(connection.id.to_string())
        .bind(&connection.name)
        .bind(connection.kind.as_str())
        .bind(&connection.endpoint)
        .bind(&connection.credential_ref)
        .bind(&connection.configuration_json)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_connection(&self, id: ConnectionId) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM connections WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_connection_configuration(
        &self,
        id: ConnectionId,
        configuration_json: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE connections SET configuration_json = ?, updated_at = ? WHERE id = ?")
            .bind(configuration_json)
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_connection(
        &self,
        connection: &ConnectionRecord,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE connections SET name = ?, endpoint = ?, credential_ref = ?, configuration_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&connection.name)
        .bind(&connection.endpoint)
        .bind(&connection.credential_ref)
        .bind(&connection.configuration_json)
        .bind(Utc::now().to_rfc3339())
        .bind(connection.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_sync_entry(&self, entry: &SyncEntryRecord) -> Result<(), DatabaseError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO sync_entries (connection_id, remote_path, state, base_fingerprint, local_fingerprint, remote_fingerprint, last_error, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(connection_id, remote_path) DO UPDATE SET state = excluded.state, base_fingerprint = excluded.base_fingerprint, local_fingerprint = excluded.local_fingerprint, remote_fingerprint = excluded.remote_fingerprint, last_error = excluded.last_error, updated_at = excluded.updated_at",
        )
        .bind(entry.connection_id.to_string())
        .bind(&entry.remote_path)
        .bind(&entry.state)
        .bind(&entry.base_fingerprint)
        .bind(&entry.local_fingerprint)
        .bind(&entry.remote_fingerprint)
        .bind(&entry.last_error)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_sync_entries(&self) -> Result<Vec<SyncEntryRecord>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT connection_id, remote_path, state, base_fingerprint, local_fingerprint, remote_fingerprint, last_error FROM sync_entries ORDER BY updated_at",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let connection_id = Uuid::parse_str(row.get::<String, _>("connection_id").as_str())
                    .map(ConnectionId::from_uuid)
                    .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
                Ok(SyncEntryRecord {
                    connection_id,
                    remote_path: row.get("remote_path"),
                    state: row.get("state"),
                    base_fingerprint: row.get("base_fingerprint"),
                    local_fingerprint: row.get("local_fingerprint"),
                    remote_fingerprint: row.get("remote_fingerprint"),
                    last_error: row.get("last_error"),
                })
            })
            .collect()
    }

    pub async fn insert_conflict(&self, conflict: &ConflictRecord) -> Result<(), DatabaseError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO conflicts (id, connection_id, remote_path, local_fingerprint, remote_fingerprint, detected_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(conflict.id.to_string())
        .bind(conflict.connection_id.to_string())
        .bind(&conflict.remote_path)
        .bind(&conflict.local_fingerprint)
        .bind(&conflict.remote_fingerprint)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_unresolved_conflicts(&self) -> Result<Vec<ConflictSummary>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, connection_id, remote_path, local_fingerprint, remote_fingerprint FROM conflicts WHERE resolution IS NULL ORDER BY detected_at",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let id = Uuid::parse_str(row.get::<String, _>("id").as_str())
                    .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
                let connection_id = Uuid::parse_str(row.get::<String, _>("connection_id").as_str())
                    .map(ConnectionId::from_uuid)
                    .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
                Ok(ConflictSummary {
                    id,
                    connection_id,
                    remote_path: row.get("remote_path"),
                    local_fingerprint: row.get("local_fingerprint"),
                    remote_fingerprint: row.get("remote_fingerprint"),
                })
            })
            .collect()
    }

    pub async fn find_conflict(&self, id: Uuid) -> Result<Option<ConflictSummary>, DatabaseError> {
        let row = sqlx::query(
            "SELECT id, connection_id, remote_path, local_fingerprint, remote_fingerprint FROM conflicts WHERE id = ? AND resolution IS NULL",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let id = Uuid::parse_str(row.get::<String, _>("id").as_str())
                .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
            let connection_id = Uuid::parse_str(row.get::<String, _>("connection_id").as_str())
                .map(ConnectionId::from_uuid)
                .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
            Ok(ConflictSummary {
                id,
                connection_id,
                remote_path: row.get("remote_path"),
                local_fingerprint: row.get("local_fingerprint"),
                remote_fingerprint: row.get("remote_fingerprint"),
            })
        })
        .transpose()
    }

    pub async fn resolve_conflict(&self, id: Uuid, resolution: &str) -> Result<(), DatabaseError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE conflicts SET resolution = ?, resolved_at = ? WHERE id = ?")
            .bind(resolution)
            .bind(now)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_activity(
        &self,
        kind: &str,
        remote_path: Option<&str>,
        status: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO activity_events (id, kind, remote_path, status, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(kind)
        .bind(remote_path)
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_activity(&self) -> Result<Vec<ActivityRecord>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, kind, remote_path, status, created_at FROM activity_events ORDER BY created_at DESC LIMIT 100",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ActivityRecord {
                    id: Uuid::parse_str(row.get::<String, _>("id").as_str())
                        .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?,
                    kind: row.get("kind"),
                    remote_path: row.get("remote_path"),
                    status: row.get("status"),
                    created_at: row.get("created_at"),
                })
            })
            .collect()
    }

    /// Records a batch that has just started and returns its row id.
    pub async fn insert_transfer_history(
        &self,
        transfer: &NewTransferHistory,
    ) -> Result<i64, DatabaseError> {
        let items = serde_json::to_string(&transfer.items)
            .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
        let mut transaction = self.pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO transfer_history \
             (started_at, mode, direction, connection, destination, items_json, item_count, state) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'running')",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(&transfer.mode)
        .bind(&transfer.direction)
        .bind(&transfer.connection)
        .bind(&transfer.destination)
        .bind(items)
        .bind(transfer.item_count)
        .execute(&mut *transaction)
        .await?
        .last_insert_rowid();
        sqlx::query(
            "DELETE FROM transfer_history WHERE id NOT IN \
             (SELECT id FROM transfer_history ORDER BY id DESC LIMIT ?)",
        )
        .bind(TRANSFER_HISTORY_LIMIT)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(id)
    }

    /// Stores how a batch ended and stamps its finish time.
    pub async fn finish_transfer_history(
        &self,
        id: i64,
        update: &TransferHistoryUpdate,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE transfer_history SET finished_at = ?, state = ?, total_files = ?, \
             done_files = ?, skipped_files = ?, failed_files = ?, total_bytes = ?, \
             done_bytes = ?, failures_json = ? WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(&update.state)
        .bind(update.total_files)
        .bind(update.done_files)
        .bind(update.skipped_files)
        .bind(update.failed_files)
        .bind(update.total_bytes)
        .bind(update.done_bytes)
        .bind(&update.failures_json)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Newest first.
    pub async fn list_transfer_history(
        &self,
        limit: i64,
    ) -> Result<Vec<TransferHistoryRecord>, DatabaseError> {
        let rows = sqlx::query("SELECT * FROM transfer_history ORDER BY id DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let items_json: String = row.get("items_json");
                Ok(TransferHistoryRecord {
                    id: row.get("id"),
                    started_at: row.get("started_at"),
                    finished_at: row.get("finished_at"),
                    mode: row.get("mode"),
                    direction: row.get("direction"),
                    connection: row.get("connection"),
                    destination: row.get("destination"),
                    items: serde_json::from_str(&items_json)
                        .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?,
                    item_count: row.get("item_count"),
                    state: row.get("state"),
                    total_files: row.get("total_files"),
                    done_files: row.get("done_files"),
                    skipped_files: row.get("skipped_files"),
                    failed_files: row.get("failed_files"),
                    total_bytes: row.get("total_bytes"),
                    done_bytes: row.get("done_bytes"),
                    failures_json: row.get("failures_json"),
                })
            })
            .collect()
    }

    /// Batches that were still running when the app stopped cannot be resumed; mark them so the
    /// log does not show them as running forever. Returns how many there were.
    pub async fn interrupt_unfinished_transfers(&self) -> Result<u64, DatabaseError> {
        let result = sqlx::query(
            "UPDATE transfer_history SET state = 'interrupted', finished_at = ? \
             WHERE finished_at IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Clears the finished entries; batches still running stay.
    pub async fn clear_finished_transfer_history(&self) -> Result<u64, DatabaseError> {
        let result = sqlx::query("DELETE FROM transfer_history WHERE finished_at IS NOT NULL")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    fn connection_from_row(
        row: sqlx::sqlite::SqliteRow,
    ) -> Result<ConnectionRecord, DatabaseError> {
        let id = Uuid::parse_str(row.get::<String, _>("id").as_str())
            .map(ConnectionId::from_uuid)
            .map_err(|error| DatabaseError::InvalidValue(format!("connection id: {error}")))?;
        let kind = ProviderKind::parse(row.get::<String, _>("provider_kind").as_str())
            .map_err(|error| DatabaseError::InvalidValue(error.to_string()))?;
        Ok(ConnectionRecord {
            id,
            name: row.get("name"),
            kind,
            endpoint: row.get("endpoint"),
            credential_ref: row.get("credential_ref"),
            configuration_json: row.get("configuration_json"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConflictRecord, ConnectionRecord, Database, NewTransferHistory, SyncEntryRecord,
        TransferHistoryUpdate, MIGRATOR, TRANSFER_HISTORY_LIMIT,
    };
    use bifrost_common::{ConnectionId, ProviderKind};
    use uuid::Uuid;

    #[tokio::test]
    async fn applies_initial_schema_to_a_fresh_database() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();

        let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'connections'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();

        assert_eq!(table_count, 1);

        let durable_state_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('cache_entries', 'transfer_queue')",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();

        assert_eq!(durable_state_count, 2);
    }

    #[tokio::test]
    async fn opens_a_file_database_from_a_native_path() {
        let database_path =
            std::env::temp_dir().join(format!("bifrost-drive-{}.db", Uuid::new_v4()));
        let database = Database::connect_file(&database_path).await.unwrap();
        database.migrate().await.unwrap();
        database.pool().close().await;
        drop(database);

        assert!(database_path.is_file());
        std::fs::remove_file(database_path).unwrap();
    }

    #[tokio::test]
    async fn repairs_checksums_from_an_older_migration_revision() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = 1")
            .bind(vec![0_u8; 48])
            .execute(database.pool())
            .await
            .unwrap();

        database.migrate().await.unwrap();

        let checksum: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 1")
                .fetch_one(database.pool())
                .await
                .unwrap();
        assert_eq!(checksum, MIGRATOR.iter().next().unwrap().checksum.as_ref());
    }

    #[tokio::test]
    async fn persists_connection_configuration_without_secret_material() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();
        let connection = ConnectionRecord {
            id: ConnectionId::new(),
            name: "Production S3".to_owned(),
            kind: ProviderKind::S3,
            endpoint: "https://s3.example.test".to_owned(),
            credential_ref: "credential-reference".to_owned(),
            configuration_json: "{}".to_owned(),
        };

        database.insert_connection(&connection).await.unwrap();
        assert_eq!(database.list_connections().await.unwrap(), vec![connection]);
    }

    #[tokio::test]
    async fn activity_history_includes_details_and_timestamp() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();

        database
            .insert_activity(
                "connection_added",
                Some("Yggdrasil · Sftp · sftp://example.test:22"),
                "completed",
            )
            .await
            .unwrap();

        let events = database.list_activity().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "connection_added");
        assert!(events[0]
            .remote_path
            .as_deref()
            .unwrap()
            .contains("Yggdrasil"));
        assert!(!events[0].created_at.is_empty());
    }

    fn started(connection: &str) -> NewTransferHistory {
        NewTransferHistory {
            mode: "copy".to_owned(),
            direction: "upload".to_owned(),
            connection: connection.to_owned(),
            destination: "Y:\\Filmer".to_owned(),
            items: vec!["C:\\Videos\\a.mkv".to_owned()],
            item_count: 1,
        }
    }

    fn ended(state: &str) -> TransferHistoryUpdate {
        TransferHistoryUpdate {
            state: state.to_owned(),
            total_files: 3,
            done_files: 2,
            skipped_files: 0,
            failed_files: 1,
            total_bytes: 3000,
            done_bytes: 2000,
            failures_json: r#"[{"name":"c.mkv","reason":{"kind":"permission_denied"}}]"#.to_owned(),
        }
    }

    #[tokio::test]
    async fn transfer_history_records_a_batch_from_start_to_finish() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();

        let id = database
            .insert_transfer_history(&started("Yggdrasil SFTP"))
            .await
            .unwrap();
        let running = &database.list_transfer_history(10).await.unwrap()[0];
        assert_eq!(running.state, "running");
        assert_eq!(running.finished_at, None);
        assert_eq!(running.items, vec!["C:\\Videos\\a.mkv"]);

        database
            .finish_transfer_history(id, &ended("completed_with_errors"))
            .await
            .unwrap();
        let finished = &database.list_transfer_history(10).await.unwrap()[0];
        assert_eq!(finished.state, "completed_with_errors");
        assert!(finished.finished_at.is_some());
        assert_eq!((finished.done_files, finished.failed_files), (2, 1));
        assert!(finished.failures_json.contains("permission_denied"));
    }

    #[tokio::test]
    async fn transfer_history_lists_newest_first_and_keeps_a_bounded_log() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();
        for number in 0..(TRANSFER_HISTORY_LIMIT + 5) {
            database
                .insert_transfer_history(&started(&format!("drive {number}")))
                .await
                .unwrap();
        }

        let all = database
            .list_transfer_history(TRANSFER_HISTORY_LIMIT + 100)
            .await
            .unwrap();

        assert_eq!(all.len() as i64, TRANSFER_HISTORY_LIMIT);
        assert_eq!(
            all[0].connection,
            format!("drive {}", TRANSFER_HISTORY_LIMIT + 4)
        );
        assert_eq!(database.list_transfer_history(3).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn unfinished_transfers_are_marked_interrupted_and_finished_ones_can_be_cleared() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();
        let done = database
            .insert_transfer_history(&started("a"))
            .await
            .unwrap();
        database
            .finish_transfer_history(done, &ended("completed"))
            .await
            .unwrap();
        database
            .insert_transfer_history(&started("b"))
            .await
            .unwrap();

        assert_eq!(database.interrupt_unfinished_transfers().await.unwrap(), 1);
        let rows = database.list_transfer_history(10).await.unwrap();
        assert_eq!(rows[0].state, "interrupted");
        assert!(rows[0].finished_at.is_some());
        assert_eq!(rows[1].state, "completed");

        database
            .insert_transfer_history(&started("c"))
            .await
            .unwrap();
        assert_eq!(database.clear_finished_transfer_history().await.unwrap(), 2);
        let left = database.list_transfer_history(10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].connection, "c");
    }

    #[tokio::test]
    async fn persists_sync_entries_and_conflicts_durably() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database.migrate().await.unwrap();
        let connection = ConnectionRecord {
            id: ConnectionId::new(),
            name: "Sync target".to_owned(),
            kind: ProviderKind::S3,
            endpoint: "https://s3.example.test".to_owned(),
            credential_ref: "native-reference".to_owned(),
            configuration_json: "{}".to_owned(),
        };
        database.insert_connection(&connection).await.unwrap();
        database
            .upsert_sync_entry(&SyncEntryRecord {
                connection_id: connection.id,
                remote_path: "docs/report.txt".to_owned(),
                state: "conflict".to_owned(),
                base_fingerprint: Some("base".to_owned()),
                local_fingerprint: Some("local".to_owned()),
                remote_fingerprint: Some("remote".to_owned()),
                last_error: None,
            })
            .await
            .unwrap();
        database
            .insert_conflict(&ConflictRecord {
                id: Uuid::new_v4(),
                connection_id: connection.id,
                remote_path: "docs/report.txt".to_owned(),
                local_fingerprint: Some("local".to_owned()),
                remote_fingerprint: Some("remote".to_owned()),
            })
            .await
            .unwrap();
        let sync_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_entries")
            .fetch_one(database.pool())
            .await
            .unwrap();
        let conflict_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conflicts")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(sync_count, 1);
        assert_eq!(conflict_count, 1);
    }
}
