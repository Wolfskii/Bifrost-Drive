use bifrost_api::{
    AppStatus, ConnectionIdRequest, ConnectionSummary, CreateConnectionRequest,
    CreateS3ConnectionRequest, CredentialSummary, FilePage, FileSummary, ListFilesRequest,
    StoreS3CredentialRequest, TestConnectionRequest,
};
use bifrost_common::{ConnectionState, ProviderKind};
use bifrost_core::Application;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
use bifrost_db::{ConnectionRecord, Database};
use bifrost_s3::{S3Config, S3Provider};
use bifrost_storage::StorageProvider;
use bifrost_windows_credentials::WindowsCredentialStore;
use serde::Deserialize;
use std::{fs, sync::Mutex};
use tauri::{Manager, State};

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

async fn test_connection(
    credentials: &WindowsCredentialStore,
    request: TestConnectionRequest,
) -> Result<(), String> {
    if request.kind != ProviderKind::S3 {
        return Err(format!(
            "{} connections are not implemented yet",
            request.kind
        ));
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
    }
    let credential: CredentialRef = serde_json::from_str(&request.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    let stored: StoredS3Credentials = serde_json::from_str(secret.expose())
        .map_err(|_| "Stored S3 credential payload is invalid".to_owned())?;
    let configuration: S3ConnectionConfiguration = serde_json::from_value(request.configuration)
        .map_err(|_| "S3 bucket configuration is invalid".to_owned())?;
    if configuration.bucket.trim().is_empty() || configuration.region.trim().is_empty() {
        return Err("S3 bucket and region are required".to_owned());
    }
    let provider = S3Provider::connect(
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
    .map_err(|error| error.to_string())?;
    provider
        .test_connection()
        .await
        .map_err(|error| error.to_string())
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
    if connection.kind != ProviderKind::S3 {
        return Err(format!(
            "{} file listing is not implemented yet",
            connection.kind
        ));
    }
    let credential: CredentialRef = serde_json::from_str(&connection.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    let stored: StoredS3Credentials = serde_json::from_str(secret.expose())
        .map_err(|_| "Stored S3 credential payload is invalid".to_owned())?;
    let configuration: S3ConnectionConfiguration =
        serde_json::from_str(&connection.configuration_json)
            .map_err(|_| "S3 bucket configuration is invalid".to_owned())?;
    let endpoint = url::Url::parse(&connection.endpoint)
        .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
    let provider = S3Provider::connect(
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
    .map_err(|error| error.to_string())?;
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
            app.manage(database);
            Ok(())
        })
        .manage(Mutex::new(Application::new()))
        .manage(WindowsCredentialStore::new())
        .invoke_handler(tauri::generate_handler![
            app_status,
            connections_list,
            connections_create,
            connections_create_s3,
            connections_remove,
            connections_test,
            files_list,
            credentials_store_s3
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bifrost Drive");
}
