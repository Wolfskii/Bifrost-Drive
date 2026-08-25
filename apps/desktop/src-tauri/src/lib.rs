use bifrost_api::{
    ActivitySummary, AppStatus, ConnectionIdRequest, ConnectionSummary, CreateConnectionRequest,
    CreateS3ConnectionRequest, CreateSftpConnectionRequest, CreateWebDavConnectionRequest,
    CredentialSummary, FilePage, FileSummary, HydrateFileRequest, HydrateFileResponse,
    ListFilesRequest, StoreS3CredentialRequest, SyncReconcileRequest, SyncReconcileResponse,
    SyncRunRequest, SyncRunResponse, TestConnectionRequest,
};
use bifrost_cache::{CacheManager, CacheRecord};
use bifrost_common::{ConnectionState, ProviderKind};
use bifrost_core::Application;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
use bifrost_db::{ConflictRecord, ConnectionRecord, Database, SyncEntryRecord};
use bifrost_s3::{S3Config, S3Provider};
use bifrost_sftp::{SftpConfig, SftpProvider};
use bifrost_storage::StorageProvider;
use bifrost_sync::{resolve, ConflictResolution, ReconciliationInput, Revision, SyncDecision};
use bifrost_transfer::TransferService;
use bifrost_transfer::{TransferDirection, TransferSnapshot, TransferStatus, TransferStore};
use bifrost_webdav::{WebDavConfig, WebDavProvider};
use bifrost_windows_cfapi::{CfapiEvent, PlaceholderMetadata, SyncRoot, SyncRootConfig};
use bifrost_windows_credentials::WindowsCredentialStore;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager, State,
};

struct SyncRootRegistry(Mutex<HashMap<String, SyncRoot>>);

struct SqliteTransferStore {
    database: Database,
}

#[async_trait::async_trait]
impl TransferStore for SqliteTransferStore {
    async fn save(&self, transfer: TransferSnapshot) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO transfer_queue (id, connection_id, remote_path, direction, status, total_bytes, transferred_bytes, attempts, next_retry_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET status = excluded.status, total_bytes = excluded.total_bytes, transferred_bytes = excluded.transferred_bytes, attempts = excluded.attempts, next_retry_at = excluded.next_retry_at, updated_at = excluded.updated_at",
        )
        .bind(transfer.id.to_string())
        .bind(transfer.connection_id.to_string())
        .bind(transfer.path.as_str())
        .bind(match transfer.direction {
            TransferDirection::Download => "download",
            TransferDirection::Upload => "upload",
        })
        .bind(match transfer.status {
            TransferStatus::Pending => "pending",
            TransferStatus::Running => "running",
            TransferStatus::Paused => "paused",
            TransferStatus::Completed => "completed",
            TransferStatus::Failed => "failed",
            TransferStatus::Cancelled => "cancelled",
        })
        .bind(transfer.total_bytes.map(|value| value as i64))
        .bind(transfer.transferred_bytes as i64)
        .bind(transfer.attempts as i64)
        .bind(transfer.next_retry_at.map(system_time_string))
        .bind(&now)
        .bind(&now)
        .execute(self.database.pool())
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    async fn load_recoverable(&self) -> Result<Vec<TransferSnapshot>, String> {
        let rows = sqlx::query(
            "SELECT id, connection_id, remote_path, direction, status, total_bytes, transferred_bytes, attempts, next_retry_at FROM transfer_queue WHERE status IN ('pending', 'running', 'paused') ORDER BY created_at",
        )
        .fetch_all(self.database.pool())
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let id = uuid::Uuid::parse_str(&sqlx::Row::get::<String, _>(&row, "id"))
                    .map(bifrost_common::TransferId::from_uuid)
                    .map_err(|error| error.to_string())?;
                let connection_id =
                    uuid::Uuid::parse_str(&sqlx::Row::get::<String, _>(&row, "connection_id"))
                        .map(bifrost_common::ConnectionId::from_uuid)
                        .map_err(|error| error.to_string())?;
                let path = bifrost_common::RemotePath::parse(&sqlx::Row::get::<String, _>(
                    &row,
                    "remote_path",
                ))
                .map_err(|error| error.to_string())?;
                let direction = match sqlx::Row::get::<String, _>(&row, "direction").as_str() {
                    "download" => TransferDirection::Download,
                    "upload" => TransferDirection::Upload,
                    value => return Err(format!("unknown transfer direction: {value}")),
                };
                let status = match sqlx::Row::get::<String, _>(&row, "status").as_str() {
                    "pending" => TransferStatus::Pending,
                    "running" => TransferStatus::Running,
                    "paused" => TransferStatus::Paused,
                    value => return Err(format!("unknown recoverable transfer status: {value}")),
                };
                Ok(TransferSnapshot {
                    id,
                    connection_id,
                    path,
                    direction,
                    status,
                    total_bytes: sqlx::Row::get::<Option<i64>, _>(&row, "total_bytes")
                        .map(|value| value as u64),
                    transferred_bytes: sqlx::Row::get::<i64, _>(&row, "transferred_bytes") as u64,
                    attempts: sqlx::Row::get::<i64, _>(&row, "attempts") as u32,
                    next_retry_at: sqlx::Row::get::<Option<String>, _>(&row, "next_retry_at")
                        .and_then(system_time_parse),
                })
            })
            .collect()
    }

    async fn save_cache(&self, record: CacheRecord) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO cache_entries (connection_id, remote_path, local_path, size_bytes, last_accessed, pinned, active_transfer) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(connection_id, remote_path) DO UPDATE SET local_path = excluded.local_path, size_bytes = excluded.size_bytes, last_accessed = excluded.last_accessed, pinned = excluded.pinned, active_transfer = excluded.active_transfer",
        )
        .bind(record.connection_id.to_string())
        .bind(record.remote_path.as_str())
        .bind(record.local_path.to_string_lossy().as_ref())
        .bind(record.size_bytes as i64)
        .bind(system_time_string(record.last_accessed))
        .bind(record.pinned as i64)
        .bind(record.active_transfer as i64)
        .execute(self.database.pool())
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    async fn load_cache(&self) -> Result<Vec<CacheRecord>, String> {
        let rows = sqlx::query(
            "SELECT connection_id, remote_path, local_path, size_bytes, last_accessed, pinned, active_transfer FROM cache_entries",
        )
        .fetch_all(self.database.pool())
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let connection_id =
                    uuid::Uuid::parse_str(&sqlx::Row::get::<String, _>(&row, "connection_id"))
                        .map(bifrost_common::ConnectionId::from_uuid)
                        .map_err(|error| error.to_string())?;
                let remote_path = bifrost_common::RemotePath::parse(&sqlx::Row::get::<String, _>(
                    &row,
                    "remote_path",
                ))
                .map_err(|error| error.to_string())?;
                let last_accessed =
                    system_time_parse(sqlx::Row::get::<String, _>(&row, "last_accessed"))
                        .ok_or_else(|| "invalid cache last_accessed timestamp".to_owned())?;
                Ok(CacheRecord {
                    connection_id,
                    remote_path,
                    local_path: PathBuf::from(sqlx::Row::get::<String, _>(&row, "local_path")),
                    size_bytes: sqlx::Row::get::<i64, _>(&row, "size_bytes") as u64,
                    last_accessed,
                    pinned: sqlx::Row::get::<i64, _>(&row, "pinned") != 0,
                    active_transfer: sqlx::Row::get::<i64, _>(&row, "active_transfer") != 0,
                })
            })
            .collect()
    }
}

fn system_time_string(value: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(value).to_rfc3339()
}

fn system_time_parse(value: String) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc).into())
}

#[tauri::command]
fn app_status(application: State<'_, Mutex<Application>>) -> AppStatus {
    application
        .lock()
        .expect("application state poisoned")
        .status()
}

#[tauri::command]
async fn connections_list(database: State<'_, Database>) -> Result<Vec<ConnectionSummary>, String> {
    database
        .list_connections()
        .await
        .map(|connections| {
            connections
                .into_iter()
                .map(|connection| ConnectionSummary {
                    id: connection.id,
                    name: connection.name,
                    kind: connection.kind,
                    state: ConnectionState::Disconnected,
                    endpoint: connection.endpoint,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn activity_list(database: State<'_, Database>) -> Result<Vec<ActivitySummary>, String> {
    database
        .list_activity()
        .await
        .map(|events| {
            events
                .into_iter()
                .map(|event| ActivitySummary {
                    id: event.id,
                    kind: event.kind,
                    remote_path: event.remote_path,
                    status: event.status,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn connections_create(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateConnectionRequest,
) -> Result<ConnectionSummary, String> {
    if request.name.trim().is_empty() || request.credential_ref.trim().is_empty() {
        return Err("Connection name and credential reference are required".to_owned());
    }
    let credential: CredentialRef = serde_json::from_str(&request.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let test_request = TestConnectionRequest {
        kind: request.kind,
        endpoint: request.endpoint.clone(),
        credential_ref: request.credential_ref.clone(),
        configuration: request.configuration.clone(),
    };
    if let Err(error) = test_connection(&credentials, test_request).await {
        let _ = credentials.delete(&credential).await;
        return Err(error);
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
    }

    let record = ConnectionRecord {
        id: bifrost_common::ConnectionId::new(),
        name: request.name,
        kind: request.kind,
        endpoint: endpoint.to_string(),
        credential_ref: serde_json::to_string(&credential)
            .map_err(|_| "Connection credential reference is invalid".to_owned())?,
        configuration_json: request.configuration.to_string(),
    };
    database
        .insert_connection(&record)
        .await
        .map_err(|error| error.to_string())?;
    Ok(ConnectionSummary {
        id: record.id,
        name: record.name,
        kind: record.kind,
        state: ConnectionState::Disconnected,
        endpoint: record.endpoint,
    })
}

#[tauri::command]
async fn connections_remove(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: ConnectionIdRequest,
) -> Result<(), String> {
    let connection = database
        .find_connection(request.id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let credential: CredentialRef = serde_json::from_str(&connection.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    match credentials.delete(&credential).await {
        Ok(()) | Err(CredentialError::NotFound) => {}
        Err(error) => return Err(error.to_string()),
    }
    database
        .delete_connection(request.id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn credentials_store_s3(
    credentials: State<'_, WindowsCredentialStore>,
    request: StoreS3CredentialRequest,
) -> Result<CredentialSummary, String> {
    if request.label.trim().is_empty() {
        return Err("Credential label is required".to_owned());
    }
    if request.access_key_id.is_empty() || request.secret_access_key.is_empty() {
        return Err("S3 access key and secret key are required".to_owned());
    }
    let payload = serde_json::json!({
        "access_key_id": request.access_key_id,
        "secret_access_key": request.secret_access_key,
    });
    let credential = credentials
        .put("s3", &request.label, SecretString::new(payload.to_string()))
        .await
        .map_err(|error| error.to_string())?;
    Ok(CredentialSummary {
        id: credential.id,
        kind: credential.kind,
        label: credential.label,
    })
}

#[tauri::command]
async fn connections_create_s3(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateS3ConnectionRequest,
) -> Result<ConnectionSummary, String> {
    if request.name.trim().is_empty()
        || request.region.trim().is_empty()
        || request.bucket.trim().is_empty()
    {
        return Err("Connection name, bucket, and region are required".to_owned());
    }
    if request.access_key_id.is_empty() || request.secret_access_key.is_empty() {
        return Err("S3 access key and secret key are required".to_owned());
    }
    let credential = credentials_store_s3(
        credentials.clone(),
        StoreS3CredentialRequest {
            label: request.name.clone(),
            access_key_id: request.access_key_id.clone(),
            secret_access_key: request.secret_access_key.clone(),
        },
    )
    .await?;
    let credential_ref = serde_json::to_string(&credential)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let configuration = serde_json::json!({
        "region": request.region,
        "bucket": request.bucket,
        "path_style": request.path_style,
    });
    let test_request = TestConnectionRequest {
        kind: ProviderKind::S3,
        endpoint: request.endpoint.clone(),
        credential_ref: credential_ref.clone(),
        configuration: configuration.clone(),
    };
    if let Err(error) = test_connection(&credentials, test_request).await {
        let _ = credentials
            .delete(&CredentialRef {
                id: credential.id,
                kind: credential.kind,
                label: credential.label,
            })
            .await;
        return Err(error);
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
    }
    let record = ConnectionRecord {
        id: bifrost_common::ConnectionId::new(),
        name: request.name,
        kind: ProviderKind::S3,
        endpoint: endpoint.to_string(),
        credential_ref,
        configuration_json: configuration.to_string(),
    };
    if let Err(error) = database.insert_connection(&record).await {
        let _ = credentials
            .delete(&CredentialRef {
                id: credential.id,
                kind: credential.kind,
                label: credential.label,
            })
            .await;
        return Err(error.to_string());
    }
    Ok(ConnectionSummary {
        id: record.id,
        name: record.name,
        kind: record.kind,
        state: ConnectionState::Connected,
        endpoint: record.endpoint,
    })
}

#[derive(Debug, Deserialize)]
struct StoredS3Credentials {
    access_key_id: String,
    secret_access_key: String,
}

#[derive(Debug, Deserialize)]
struct S3ConnectionConfiguration {
    region: String,
    bucket: String,
    path_style: bool,
}

#[derive(Debug, Deserialize)]
struct WebDavCredentials {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct SftpConfiguration {
    host: String,
    port: u16,
    known_hosts: String,
    #[serde(default = "default_sftp_authentication")]
    authentication: String,
    private_key_path: Option<String>,
}

fn default_sftp_authentication() -> String {
    "password".to_owned()
}

#[derive(Debug, Deserialize)]
struct SftpCredentials {
    username: String,
    password: Option<String>,
    private_key_path: Option<String>,
    passphrase: Option<String>,
}

async fn test_connection(
    credentials: &WindowsCredentialStore,
    request: TestConnectionRequest,
) -> Result<(), String> {
    let credential: CredentialRef = serde_json::from_str(&request.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    match request.kind {
        ProviderKind::S3 => {
            let endpoint = url::Url::parse(&request.endpoint)
                .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
            if !matches!(endpoint.scheme(), "http" | "https") {
                return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
            }
            let stored: StoredS3Credentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored S3 credential payload is invalid".to_owned())?;
            let configuration: S3ConnectionConfiguration =
                serde_json::from_value(request.configuration)
                    .map_err(|_| "S3 bucket configuration is invalid".to_owned())?;
            if configuration.bucket.trim().is_empty() || configuration.region.trim().is_empty() {
                return Err("S3 bucket and region are required".to_owned());
            }
            S3Provider::connect(
                S3Config {
                    endpoint,
                    region: configuration.region,
                    bucket: configuration.bucket,
                    path_style: configuration.path_style,
                },
                stored.access_key_id,
                stored.secret_access_key,
            )
            .await
            .map_err(|error| error.to_string())?
            .test_connection()
            .await
            .map_err(|error| error.to_string())
        }
        ProviderKind::WebDav | ProviderKind::Nextcloud => {
            let endpoint = url::Url::parse(&request.endpoint)
                .map_err(|_| "WebDAV endpoint must be a valid URL".to_owned())?;
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored WebDAV credential payload is invalid".to_owned())?;
            WebDavProvider::connect(
                WebDavConfig {
                    endpoint,
                    username: stored.username,
                },
                stored.password,
            )
            .map_err(|error| error.to_string())?
            .test_connection()
            .await
            .map_err(|error| error.to_string())
        }
        ProviderKind::Sftp => {
            let configuration: SftpConfiguration = serde_json::from_value(request.configuration)
                .map_err(|_| "SFTP configuration is invalid".to_owned())?;
            let stored: SftpCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored SFTP credential payload is invalid".to_owned())?;
            let config = SftpConfig {
                host: configuration.host,
                port: configuration.port,
                username: stored.username,
                known_hosts: configuration.known_hosts.into(),
            };
            let provider = (if configuration.authentication == "private_key" {
                let key_path = configuration
                    .private_key_path
                    .or(stored.private_key_path)
                    .ok_or_else(|| "SFTP private key path is required".to_owned())?;
                SftpProvider::connect_with_private_key(config, key_path.into(), stored.passphrase)
            } else {
                SftpProvider::connect(
                    config,
                    stored
                        .password
                        .ok_or_else(|| "SFTP password is required".to_owned())?,
                )
            })
            .map_err(|error| error.to_string())?;
            provider
                .test_connection()
                .await
                .map_err(|error| error.to_string())
        }
    }
}

async fn create_tested_connection(
    database: &Database,
    credentials: &WindowsCredentialStore,
    name: String,
    kind: ProviderKind,
    endpoint: String,
    configuration: serde_json::Value,
    secret_payload: serde_json::Value,
) -> Result<ConnectionSummary, String> {
    let credential = credentials
        .put(
            "connection",
            &name,
            SecretString::new(secret_payload.to_string()),
        )
        .await
        .map_err(|error| error.to_string())?;
    let credential_ref = serde_json::to_string(&credential)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    if let Err(error) = test_connection(
        credentials,
        TestConnectionRequest {
            kind,
            endpoint: endpoint.clone(),
            credential_ref: credential_ref.clone(),
            configuration: configuration.clone(),
        },
    )
    .await
    {
        let _ = credentials.delete(&credential).await;
        return Err(error);
    }
    let record = ConnectionRecord {
        id: bifrost_common::ConnectionId::new(),
        name,
        kind,
        endpoint,
        credential_ref,
        configuration_json: configuration.to_string(),
    };
    if let Err(error) = database.insert_connection(&record).await {
        let _ = credentials.delete(&credential).await;
        return Err(error.to_string());
    }
    Ok(ConnectionSummary {
        id: record.id,
        name: record.name,
        kind: record.kind,
        state: ConnectionState::Connected,
        endpoint: record.endpoint,
    })
}

async fn provider_for_connection(
    connection: &ConnectionRecord,
    credentials: &WindowsCredentialStore,
) -> Result<Box<dyn StorageProvider>, String> {
    let credential: CredentialRef = serde_json::from_str(&connection.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    match connection.kind {
        ProviderKind::S3 => {
            let stored: StoredS3Credentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored S3 credential payload is invalid".to_owned())?;
            let configuration: S3ConnectionConfiguration =
                serde_json::from_str(&connection.configuration_json)
                    .map_err(|_| "S3 bucket configuration is invalid".to_owned())?;
            let endpoint = url::Url::parse(&connection.endpoint)
                .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
            Ok(Box::new(
                S3Provider::connect(
                    S3Config {
                        endpoint,
                        region: configuration.region,
                        bucket: configuration.bucket,
                        path_style: configuration.path_style,
                    },
                    stored.access_key_id,
                    stored.secret_access_key,
                )
                .await
                .map_err(|error| error.to_string())?,
            ))
        }
        ProviderKind::WebDav | ProviderKind::Nextcloud => {
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored WebDAV credential payload is invalid".to_owned())?;
            let endpoint = url::Url::parse(&connection.endpoint)
                .map_err(|_| "WebDAV endpoint must be a valid URL".to_owned())?;
            Ok(Box::new(
                WebDavProvider::connect(
                    WebDavConfig {
                        endpoint,
                        username: stored.username,
                    },
                    stored.password,
                )
                .map_err(|error| error.to_string())?,
            ))
        }
        ProviderKind::Sftp => {
            let configuration: SftpConfiguration =
                serde_json::from_str(&connection.configuration_json)
                    .map_err(|_| "SFTP configuration is invalid".to_owned())?;
            let stored: SftpCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored SFTP credential payload is invalid".to_owned())?;
            let config = SftpConfig {
                host: configuration.host,
                port: configuration.port,
                username: stored.username,
                known_hosts: configuration.known_hosts.into(),
            };
            if configuration.authentication == "private_key" {
                let key_path = configuration
                    .private_key_path
                    .or(stored.private_key_path)
                    .ok_or_else(|| "SFTP private key path is required".to_owned())?;
                Ok(Box::new(
                    SftpProvider::connect_with_private_key(
                        config,
                        key_path.into(),
                        stored.passphrase,
                    )
                    .map_err(|error| error.to_string())?,
                ))
            } else {
                Ok(Box::new(
                    SftpProvider::connect(
                        config,
                        stored
                            .password
                            .ok_or_else(|| "SFTP password is required".to_owned())?,
                    )
                    .map_err(|error| error.to_string())?,
                ))
            }
        }
    }
}

#[tauri::command]
async fn connections_create_webdav(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateWebDavConnectionRequest,
) -> Result<ConnectionSummary, String> {
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "WebDAV endpoint must be a valid URL".to_owned())?;
    create_tested_connection(
        &database,
        &credentials,
        request.name,
        ProviderKind::WebDav,
        endpoint.to_string(),
        serde_json::json!({}),
        serde_json::json!({ "username": request.username, "password": request.password }),
    )
    .await
}

#[tauri::command]
async fn connections_create_sftp(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateSftpConnectionRequest,
) -> Result<ConnectionSummary, String> {
    if request.host.trim().is_empty()
        || request.username.trim().is_empty()
        || request.known_hosts.trim().is_empty()
    {
        return Err("SFTP host, username, and known_hosts path are required".to_owned());
    }
    if request.authentication == "private_key"
        && request
            .private_key_path
            .as_deref()
            .is_none_or(|path| path.trim().is_empty())
    {
        return Err("SFTP private key path is required".to_owned());
    }
    if request.authentication != "password" && request.authentication != "private_key" {
        return Err("SFTP authentication must be password or private_key".to_owned());
    }
    let authentication = request.authentication.clone();
    create_tested_connection(
        &database,
        &credentials,
        request.name,
        ProviderKind::Sftp,
        format!("sftp://{}:{}", request.host, request.port),
        serde_json::json!({ "host": request.host, "port": request.port, "username": request.username, "known_hosts": request.known_hosts, "authentication": authentication, "private_key_path": request.private_key_path }),
        serde_json::json!({ "username": request.username, "password": (request.authentication == "password").then_some(request.password), "private_key_path": request.private_key_path, "passphrase": request.passphrase }),
    )
    .await
}

#[tauri::command]
async fn connections_test(
    credentials: State<'_, WindowsCredentialStore>,
    request: TestConnectionRequest,
) -> Result<(), String> {
    test_connection(&credentials, request).await
}

#[tauri::command]
async fn files_list(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: ListFilesRequest,
) -> Result<FilePage, String> {
    let connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let provider = provider_for_connection(&connection, &credentials).await?;
    let page = provider
        .list(&request.path, request.cursor.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    Ok(FilePage {
        entries: page
            .entries
            .into_iter()
            .map(|entry| FileSummary {
                path: entry.metadata.path,
                is_directory: entry.metadata.is_directory,
                size_bytes: entry.metadata.size_bytes,
                modified_at: entry.metadata.modified_at.map(|value| value.to_rfc3339()),
            })
            .collect(),
        next_cursor: page.next_cursor,
    })
}

#[tauri::command]
async fn files_hydrate(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: HydrateFileRequest,
) -> Result<HydrateFileResponse, String> {
    let connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let provider = provider_for_connection(&connection, &credentials).await?;
    let metadata = provider
        .stat(&request.path)
        .await
        .map_err(|error| error.to_string())?;
    let local_path = transfers
        .hydrate(
            provider.as_ref(),
            request.connection_id,
            request.path.clone(),
            metadata.size_bytes,
            request.pinned,
        )
        .await
        .map_err(|error| error.to_string())?;
    database
        .insert_activity("hydrate", Some(request.path.as_str()), "completed")
        .await
        .map_err(|error| error.to_string())?;
    Ok(HydrateFileResponse {
        path: request.path,
        local_path: local_path.display().to_string(),
    })
}

#[tauri::command]
fn sync_reconcile(request: SyncReconcileRequest) -> Result<SyncReconcileResponse, String> {
    let input = ReconciliationInput {
        base: request.base.map(Revision::new),
        local: request.local.map(Revision::new),
        remote: request.remote.map(Revision::new),
    };
    let resolution = match request.resolution.as_deref() {
        None => None,
        Some("keep_local") => Some(ConflictResolution::KeepLocal),
        Some("keep_remote") => Some(ConflictResolution::KeepRemote),
        Some("keep_both") => Some(ConflictResolution::KeepBoth),
        Some("rename_conflict") => Some(ConflictResolution::RenameConflict),
        Some(value) => return Err(format!("unknown conflict resolution: {value}")),
    };
    let decision = resolve(&input, resolution).map_err(|error| error.to_string())?;
    let conflict = matches!(decision, SyncDecision::Conflict);
    Ok(SyncReconcileResponse {
        decision: format!("{decision:?}"),
        conflict,
    })
}

#[tauri::command]
async fn sync_run(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: SyncRunRequest,
) -> Result<SyncRunResponse, String> {
    run_sync(&database, &credentials, &transfers, request).await
}

async fn run_sync(
    database: &Database,
    credentials: &WindowsCredentialStore,
    transfers: &TransferService,
    request: SyncRunRequest,
) -> Result<SyncRunResponse, String> {
    let connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let provider = provider_for_connection(&connection, credentials).await?;
    let remote = provider
        .stat(&request.path)
        .await
        .map_err(|error| error.to_string())?;
    let remote_fingerprint = remote.etag.clone().unwrap_or_else(|| {
        format!(
            "{}:{:?}",
            remote.size_bytes.unwrap_or_default(),
            remote.modified_at
        )
    });
    let input = ReconciliationInput {
        base: request.base.clone().map(Revision::new),
        local: request.local.clone().map(Revision::new),
        remote: Some(Revision::new(remote_fingerprint.clone())),
    };
    let resolution = parse_resolution(request.resolution.as_deref())?;
    let decision = resolve(&input, resolution).map_err(|error| error.to_string())?;
    let mut state = format!("{decision:?}").to_lowercase();
    let mut base_fingerprint = request.base.clone();
    let mut local_fingerprint = request.local.clone();
    let mut conflict_id = None;
    match decision {
        SyncDecision::DownloadRemote => {
            transfers
                .hydrate(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                    remote.size_bytes,
                    false,
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = Some(remote_fingerprint.clone());
            local_fingerprint = Some(remote_fingerprint.clone());
        }
        SyncDecision::UploadLocal => {
            transfers
                .upload_cached(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = Some(remote_fingerprint.clone());
            local_fingerprint = request
                .local
                .clone()
                .or_else(|| Some(remote_fingerprint.clone()));
        }
        SyncDecision::Conflict => {
            let id = uuid::Uuid::new_v4();
            database
                .insert_conflict(&ConflictRecord {
                    id,
                    connection_id: request.connection_id,
                    remote_path: request.path.to_string(),
                    local_fingerprint: request.local.clone(),
                    remote_fingerprint: Some(remote_fingerprint.clone()),
                })
                .await
                .map_err(|error| error.to_string())?;
            conflict_id = Some(id);
            state = "conflict".to_owned();
        }
        SyncDecision::Resolved(ConflictResolution::KeepRemote) => {
            transfers
                .hydrate(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                    remote.size_bytes,
                    false,
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = Some(remote_fingerprint.clone());
            local_fingerprint = Some(remote_fingerprint.clone());
        }
        SyncDecision::Resolved(ConflictResolution::KeepLocal) => {
            transfers
                .upload_cached(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = request
                .local
                .clone()
                .or_else(|| Some(remote_fingerprint.clone()));
            local_fingerprint = base_fingerprint.clone();
        }
        SyncDecision::Resolved(ConflictResolution::KeepBoth)
        | SyncDecision::Resolved(ConflictResolution::RenameConflict) => {
            return Err(
                "this conflict resolution requires a materialized conflict filename and is not available yet"
                    .to_owned(),
            );
        }
        SyncDecision::UpToDate | SyncDecision::DeleteLocal | SyncDecision::DeleteRemote => {}
    }
    database
        .upsert_sync_entry(&SyncEntryRecord {
            connection_id: request.connection_id,
            remote_path: request.path.to_string(),
            state: state.clone(),
            base_fingerprint,
            local_fingerprint,
            remote_fingerprint: Some(remote_fingerprint),
            last_error: None,
        })
        .await
        .map_err(|error| error.to_string())?;
    database
        .insert_activity("sync", Some(request.path.as_str()), &state)
        .await
        .map_err(|error| error.to_string())?;
    Ok(SyncRunResponse {
        decision: state.clone(),
        conflict: state == "conflict",
        conflict_id,
    })
}

fn parse_resolution(value: Option<&str>) -> Result<Option<ConflictResolution>, String> {
    match value {
        None => Ok(None),
        Some("keep_local") => Ok(Some(ConflictResolution::KeepLocal)),
        Some("keep_remote") => Ok(Some(ConflictResolution::KeepRemote)),
        Some("keep_both") => Ok(Some(ConflictResolution::KeepBoth)),
        Some("rename_conflict") => Ok(Some(ConflictResolution::RenameConflict)),
        Some(value) => Err(format!("unknown conflict resolution: {value}")),
    }
}

#[tauri::command]
async fn sync_conflict_resolve(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: bifrost_api::ResolveConflictRequest,
) -> Result<(), String> {
    let resolution = parse_resolution(Some(&request.resolution))?
        .ok_or_else(|| "a conflict resolution is required".to_owned())?;
    if matches!(
        resolution,
        ConflictResolution::KeepBoth | ConflictResolution::RenameConflict
    ) {
        return Err(
            "this conflict resolution requires a materialized conflict filename and is not available yet"
                .to_owned(),
        );
    }
    let conflict = database
        .find_conflict(request.id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Conflict was not found or is already resolved".to_owned())?;
    let path = bifrost_common::RemotePath::parse(&conflict.remote_path)
        .map_err(|error| error.to_string())?;
    run_sync(
        &database,
        &credentials,
        &transfers,
        SyncRunRequest {
            connection_id: conflict.connection_id,
            path,
            base: Some("__conflict__".to_owned()),
            local: conflict.local_fingerprint.clone(),
            resolution: Some(request.resolution.clone()),
        },
    )
    .await?;
    database
        .resolve_conflict(request.id, &request.resolution)
        .await
        .map_err(|error| error.to_string())?;
    database
        .insert_activity("conflict", Some(&conflict.remote_path), &request.resolution)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn sync_conflicts_list(
    database: State<'_, Database>,
) -> Result<Vec<bifrost_api::ConflictSummary>, String> {
    database
        .list_unresolved_conflicts()
        .await
        .map(|conflicts| {
            conflicts
                .into_iter()
                .map(|conflict| bifrost_api::ConflictSummary {
                    id: conflict.id,
                    connection_id: conflict.connection_id,
                    remote_path: conflict.remote_path,
                    local_fingerprint: conflict.local_fingerprint,
                    remote_fingerprint: conflict.remote_fingerprint,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

fn start_sync_scheduler(database: Database, transfers: Arc<TransferService>) {
    tauri::async_runtime::spawn(async move {
        let credentials = WindowsCredentialStore::new();
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            let entries = match database.list_sync_entries().await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries {
                if entry.state == "conflict" {
                    continue;
                }
                let Ok(path) = bifrost_common::RemotePath::parse(&entry.remote_path) else {
                    continue;
                };
                let _ = run_sync(
                    &database,
                    &credentials,
                    &transfers,
                    SyncRunRequest {
                        connection_id: entry.connection_id,
                        path,
                        base: entry.base_fingerprint,
                        local: entry.local_fingerprint,
                        resolution: None,
                    },
                )
                .await;
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn cfapi_handler(
    provider: Arc<dyn StorageProvider>,
    transfers: Arc<TransferService>,
    connection_id: bifrost_common::ConnectionId,
) -> Arc<dyn Fn(CfapiEvent) + Send + Sync> {
    Arc::new(move |event| {
        let provider = Arc::clone(&provider);
        let transfers = Arc::clone(&transfers);
        let _ = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async move {
                match &event {
                    CfapiEvent::FetchData {
                        file_identity,
                        file_offset,
                        required_length,
                        ..
                    } => {
                        let path = match bifrost_common::RemotePath::parse(
                            &String::from_utf8_lossy(file_identity),
                        ) {
                            Ok(path) => path,
                            Err(_) => return,
                        };
                        let end = file_offset.saturating_add(*required_length);
                        let range =
                            (*required_length > 0).then_some(*file_offset as u64..end as u64);
                        let mut stream = match provider
                            .read(bifrost_storage::ReadRequest { path, range })
                            .await
                        {
                            Ok(stream) => stream,
                            Err(_) => return,
                        };
                        let mut offset = *file_offset;
                        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                            let chunk = match chunk {
                                Ok(chunk) => chunk,
                                Err(_) => return,
                            };
                            if SyncRoot::complete_fetch_data(&event, offset, &chunk).is_err() {
                                return;
                            }
                            offset = offset.saturating_add(chunk.len() as i64);
                        }
                    }
                    CfapiEvent::FetchPlaceholders { file_identity, .. } => {
                        let parent = match bifrost_common::RemotePath::parse(
                            &String::from_utf8_lossy(file_identity),
                        ) {
                            Ok(path) => path,
                            Err(_) => return,
                        };
                        let page = match provider.list(&parent, None).await {
                            Ok(page) => page,
                            Err(_) => return,
                        };
                        let entries = page
                            .entries
                            .into_iter()
                            .map(|entry| {
                                let path = entry.metadata.path.as_str();
                                let relative = if parent.as_str().is_empty() {
                                    path.to_owned()
                                } else {
                                    path.strip_prefix(&format!("{}/", parent.as_str()))
                                        .unwrap_or(path)
                                        .to_owned()
                                };
                                PlaceholderMetadata {
                                    relative_name: relative,
                                    identity: entry.metadata.path.as_str().as_bytes().to_vec(),
                                    remote: entry.metadata,
                                }
                            })
                            .collect::<Vec<_>>();
                        let _ = SyncRoot::complete_fetch_placeholders(&event, &entries);
                    }
                    CfapiEvent::NotifyFileClose { file_identity } => {
                        let Ok(path) = bifrost_common::RemotePath::parse(&String::from_utf8_lossy(
                            file_identity,
                        )) else {
                            return;
                        };
                        let _ = transfers
                            .upload_cached(provider.as_ref(), connection_id, path)
                            .await;
                    }
                    CfapiEvent::NotifyDelete { file_identity } => {
                        let Ok(path) = bifrost_common::RemotePath::parse(&String::from_utf8_lossy(
                            file_identity,
                        )) else {
                            return;
                        };
                        let _ = provider.delete(&path).await;
                    }
                    CfapiEvent::NotifyRename {
                        file_identity,
                        target_path,
                    } => {
                        let Ok(path) = bifrost_common::RemotePath::parse(&String::from_utf8_lossy(
                            file_identity,
                        )) else {
                            return;
                        };
                        let Ok(target) = bifrost_common::RemotePath::parse(target_path) else {
                            return;
                        };
                        let _ = provider.rename(&path, &target).await;
                    }
                }
            });
        })
        .join();
    })
}

#[tauri::command]
async fn sync_root_register(
    registry: State<'_, SyncRootRegistry>,
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: bifrost_api::SyncRootRegisterRequest,
) -> Result<bifrost_api::SyncRootRegisterResponse, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (registry, database, credentials, transfers, request);
        return Err("Windows CFAPI is available only on Windows".to_owned());
    }
    #[cfg(target_os = "windows")]
    {
        let connection = database
            .find_connection(request.connection_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Connection was not found".to_owned())?;
        let provider = provider_for_connection(&connection, &credentials).await?;
        let path = PathBuf::from(&request.path);
        if !path.is_absolute() {
            return Err("Sync root path must be absolute".to_owned());
        }
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        let mut provider_id = [0; 16];
        provider_id.copy_from_slice(connection.id.as_uuid().as_bytes());
        let root = SyncRoot::register(SyncRootConfig {
            path: path.clone(),
            provider_name: "Bifrost Drive".to_owned(),
            provider_version: env!("CARGO_PKG_VERSION").to_owned(),
            provider_id,
            sync_root_identity: request.path.as_bytes().to_vec(),
            root_file_identity: b"root".to_vec(),
        })
        .map_err(|error| error.to_string())?
        .connect_with_handler(cfapi_handler(
            Arc::from(provider),
            Arc::clone(&transfers),
            request.connection_id,
        ))
        .map_err(|error| error.to_string())?;
        registry
            .0
            .lock()
            .expect("sync root registry poisoned")
            .insert(request.path.clone(), root);
        Ok(bifrost_api::SyncRootRegisterResponse { path: request.path })
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .compact()
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let database_path = data_dir.join("bifrost-drive.db");
            let database = tauri::async_runtime::block_on(Database::connect_file(&database_path))?;
            tauri::async_runtime::block_on(database.migrate())?;
            let cache = CacheManager::new(data_dir.join("cache"), 1024 * 1024 * 1024)?;
            let transfer_store = Arc::new(SqliteTransferStore {
                database: database.clone(),
            });
            let transfers = TransferService::with_store(cache, 4, 5, Some(transfer_store));
            tauri::async_runtime::block_on(transfers.recover())?;
            let transfers = Arc::new(transfers);
            start_sync_scheduler(database.clone(), Arc::clone(&transfers));
            app.manage(database);
            app.manage(transfers);
            let open = MenuItemBuilder::with_id("open", "Open Bifrost Drive").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;
            TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .manage(Mutex::new(Application::new()))
        .manage(WindowsCredentialStore::new())
        .manage(SyncRootRegistry(Mutex::new(HashMap::new())))
        .invoke_handler(tauri::generate_handler![
            app_status,
            connections_list,
            activity_list,
            connections_create,
            connections_create_s3,
            connections_create_webdav,
            connections_create_sftp,
            connections_remove,
            connections_test,
            files_list,
            files_hydrate,
            credentials_store_s3,
            sync_reconcile,
            sync_run,
            sync_conflicts_list,
            sync_conflict_resolve,
            sync_root_register
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bifrost Drive");
}
