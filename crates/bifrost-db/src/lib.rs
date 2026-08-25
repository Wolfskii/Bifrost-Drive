use bifrost_common::{ConnectionId, ProviderKind};
use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

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
}

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
        sqlx::migrate!("./migrations").run(&self.pool).await?;
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
            "SELECT id, kind, remote_path, status FROM activity_events ORDER BY created_at DESC LIMIT 100",
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
                })
            })
            .collect()
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
    use super::{ConflictRecord, ConnectionRecord, Database, SyncEntryRecord};
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
