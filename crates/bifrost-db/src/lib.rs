use bifrost_common::{ConnectionId, ProviderKind};
use chrono::Utc;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
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
        let database_url = format!("sqlite://{}", path.display());
        Self::connect(&database_url).await
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
    use super::{ConnectionRecord, Database};
    use bifrost_common::{ConnectionId, ProviderKind};

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
}
