use bifrost_api::{
    AppStatus, ConnectionIdRequest, ConnectionSummary, CreateConnectionRequest, CredentialSummary,
    StoreS3CredentialRequest,
};
use bifrost_common::ConnectionState;
use bifrost_core::Application;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
use bifrost_db::{ConnectionRecord, Database};
use bifrost_windows_credentials::WindowsCredentialStore;
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
    request: CreateConnectionRequest,
) -> Result<ConnectionSummary, String> {
    if request.name.trim().is_empty() || request.credential_ref.trim().is_empty() {
        return Err("Connection name and credential reference are required".to_owned());
    }
    let credential: CredentialRef = serde_json::from_str(&request.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
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
            connections_remove,
            credentials_store_s3
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bifrost Drive");
}
