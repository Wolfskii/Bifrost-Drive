#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    path::{Path, PathBuf},
    time::Duration,
};

use bifrost_common::{Capability, CapabilitySet, ConnectionId, RemoteMetadata, RemotePath};
use bifrost_db::{ConnectionRecord, Database};
use bifrost_macos_credentials::MacosCredentialStore;
use bifrost_storage::{ReadRequest, StorageError, WriteRequest};
use futures_util::StreamExt;
use libloading::Library;
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncWriteExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

const BRIDGE_LIBRARY: &str = "libBifrostFileProviderHostBridge.dylib";
const POLL_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, thiserror::Error)]
enum BrokerFailure {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRequest {
    id: String,
    operation: String,
    connection_id: String,
    identifier: Option<String>,
    parent_identifier: Option<String>,
    filename: Option<String>,
    is_directory: Option<bool>,
    page_token: Option<String>,
    content_file: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<RemoteItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Vec<RemoteItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_page_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BrokerError>,
}

#[derive(Debug, Serialize)]
struct BrokerError {
    code: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteItem {
    identifier: String,
    parent_identifier: String,
    filename: String,
    is_directory: bool,
    size: Option<i64>,
    modified_at: Option<String>,
    capabilities: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesiredDomain {
    identifier: String,
    display_name: String,
}

pub fn start(database: Database) -> Result<(), String> {
    let root = bridge_group_container()?.join("FileProvider");
    std::fs::create_dir_all(root.join("requests")).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(root.join("responses")).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(root.join("payloads")).map_err(|error| error.to_string())?;

    tauri::async_runtime::spawn(async move {
        let credentials = MacosCredentialStore::new();
        for attempt in 1..=3 {
            match sync_domains(&database).await {
                Ok(()) => break,
                Err(error) if attempt < 3 => {
                    tracing::warn!(%error, %attempt, "retrying macOS File Provider domain synchronization");
                    tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
                }
                Err(error) => {
                    tracing::error!(%error, "could not synchronize macOS File Provider domains");
                }
            }
        }
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Err(error) = process_requests(&root, &database, &credentials).await {
                tracing::warn!(%error, "macOS File Provider request processing failed");
            }
        }
    });
    Ok(())
}

fn bridge_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let macos = executable
        .parent()
        .ok_or_else(|| "macOS executable has no parent directory".to_owned())?;
    let bundled = macos.join("../Frameworks").join(BRIDGE_LIBRARY);
    if bundled.is_file() {
        return Ok(bundled);
    }
    let staged = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/macos-file-provider")
        .join(BRIDGE_LIBRARY);
    if staged.is_file() {
        return Ok(staged);
    }
    Err(format!(
        "macOS File Provider bridge was not found at {} or {}",
        bundled.display(),
        staged.display()
    ))
}

fn bridge_group_container() -> Result<PathBuf, String> {
    type GroupContainer = unsafe extern "C" fn(*mut c_char, usize) -> i32;
    let library = unsafe { Library::new(bridge_path()?) }.map_err(|error| error.to_string())?;
    let function =
        unsafe { library.get::<GroupContainer>(b"bifrost_file_provider_group_container") }
            .map_err(|error| error.to_string())?;
    let mut buffer = vec![0 as c_char; 4096];
    let status = unsafe { function(buffer.as_mut_ptr(), buffer.len()) };
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if status == 0 {
        Ok(PathBuf::from(message))
    } else {
        Err(message)
    }
}

async fn sync_domains(database: &Database) -> Result<(), String> {
    type SyncDomains = unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32;
    let domains = database
        .list_connections()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|connection| mount_on_startup(&connection.configuration_json))
        .map(|connection| DesiredDomain {
            identifier: connection.id.to_string(),
            display_name: connection.name,
        })
        .collect::<Vec<_>>();
    let payload = CString::new(serde_json::to_string(&domains).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let library = unsafe { Library::new(bridge_path()?) }.map_err(|error| error.to_string())?;
        let function = unsafe { library.get::<SyncDomains>(b"bifrost_file_provider_sync_domains") }
            .map_err(|error| error.to_string())?;
        call_domain_function(|error, capacity| unsafe {
            function(payload.as_ptr(), error, capacity)
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn register_domain(
    database: &Database,
    connection_id: ConnectionId,
) -> Result<String, String> {
    type AddDomain = unsafe extern "C" fn(*const c_char, *const c_char, *mut c_char, usize) -> i32;
    let connection = database
        .find_connection(connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let identifier = CString::new(connection.id.to_string()).map_err(|error| error.to_string())?;
    let display_name = CString::new(connection.name.clone()).map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let library = unsafe { Library::new(bridge_path()?) }.map_err(|error| error.to_string())?;
        let function = unsafe { library.get::<AddDomain>(b"bifrost_file_provider_add_domain") }
            .map_err(|error| error.to_string())?;
        call_domain_function(|error, capacity| unsafe {
            function(identifier.as_ptr(), display_name.as_ptr(), error, capacity)
        })
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(format!("Finder > Locations > {}", connection.name))
}

pub async fn remove_domain(connection_id: ConnectionId) -> Result<(), String> {
    type RemoveDomain = unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32;
    let identifier = CString::new(connection_id.to_string()).map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let library = unsafe { Library::new(bridge_path()?) }.map_err(|error| error.to_string())?;
        let function =
            unsafe { library.get::<RemoveDomain>(b"bifrost_file_provider_remove_domain") }
                .map_err(|error| error.to_string())?;
        call_domain_function(|error, capacity| unsafe {
            function(identifier.as_ptr(), error, capacity)
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn open_domain(connection_id: ConnectionId) -> Result<(), String> {
    type OpenDomain = unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32;
    let identifier = CString::new(connection_id.to_string()).map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let library = unsafe { Library::new(bridge_path()?) }.map_err(|error| error.to_string())?;
        let function = unsafe { library.get::<OpenDomain>(b"bifrost_file_provider_open_domain") }
            .map_err(|error| error.to_string())?;
        call_domain_function(|error, capacity| unsafe {
            function(identifier.as_ptr(), error, capacity)
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn call_domain_function(call: impl FnOnce(*mut c_char, usize) -> i32) -> Result<(), String> {
    let mut error_buffer = vec![0 as c_char; 4096];
    let status = call(error_buffer.as_mut_ptr(), error_buffer.len());
    if status == 0 {
        Ok(())
    } else {
        Err(unsafe { CStr::from_ptr(error_buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned())
    }
}

fn mount_on_startup(configuration_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(configuration_json)
        .ok()
        .and_then(|value| {
            value
                .get("mount_on_startup")
                .and_then(|value| value.as_bool())
        })
        .unwrap_or(true)
}

async fn process_requests(
    root: &Path,
    database: &Database,
    credentials: &MacosCredentialStore,
) -> Result<(), String> {
    let requests = root.join("requests");
    let mut entries = fs::read_dir(&requests)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let processing = path.with_extension("processing");
        if fs::rename(&path, &processing).await.is_err() {
            continue;
        }
        let response = match fs::read(&processing).await {
            Ok(data) => match serde_json::from_slice::<BrokerRequest>(&data) {
                Ok(request) => {
                    let response_path = root.join("responses").join(format!("{}.json", request.id));
                    let response = handle_request(root, database, credentials, request).await;
                    write_response(&response_path, &response).await
                }
                Err(error) => Err(error.to_string()),
            },
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = response {
            tracing::warn!(%error, path = %processing.display(), "could not answer File Provider request");
        }
        let _ = fs::remove_file(processing).await;
    }
    Ok(())
}

async fn write_response(path: &Path, response: &BrokerResponse) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let data = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    fs::write(&temporary, data)
        .await
        .map_err(|error| error.to_string())?;
    fs::rename(temporary, path)
        .await
        .map_err(|error| error.to_string())
}

async fn handle_request(
    root: &Path,
    database: &Database,
    credentials: &MacosCredentialStore,
    request: BrokerRequest,
) -> BrokerResponse {
    match handle_request_inner(root, database, credentials, &request).await {
        Ok(response) => response,
        Err(error) => BrokerResponse {
            ok: false,
            item: None,
            items: None,
            next_page_token: None,
            content_file: None,
            error: Some(BrokerError {
                code: error_code(&error).to_owned(),
                message: error.to_string(),
            }),
        },
    }
}

async fn handle_request_inner(
    root: &Path,
    database: &Database,
    credentials: &MacosCredentialStore,
    request: &BrokerRequest,
) -> Result<BrokerResponse, BrokerFailure> {
    let connection_id = Uuid::parse_str(&request.connection_id)
        .map(ConnectionId::from_uuid)
        .map_err(message_error)?;
    let connection = database
        .find_connection(connection_id)
        .await
        .map_err(message_error)?
        .ok_or_else(|| message_error("connection was not found"))?;
    let provider = crate::provider_for_connection(&connection, credentials)
        .await
        .map_err(message_error)?;
    match request.operation.as_str() {
        "item" => {
            let path = request_path(request.identifier.as_deref())?;
            let metadata = if path.as_str().is_empty() {
                root_metadata()
            } else {
                provider.stat(&path).await?
            };
            Ok(item_response(remote_item(
                &connection,
                metadata,
                &provider.capabilities_for_path(&path),
            )))
        }
        "enumerate" => {
            let path = request_path(request.parent_identifier.as_deref())?;
            let page = provider.list(&path, request.page_token.as_deref()).await?;
            Ok(BrokerResponse {
                ok: true,
                item: None,
                items: Some(
                    page.entries
                        .into_iter()
                        .map(|entry| {
                            let capabilities = provider.capabilities_for_path(&entry.metadata.path);
                            remote_item(&connection, entry.metadata, &capabilities)
                        })
                        .collect(),
                ),
                next_page_token: page.next_cursor,
                content_file: None,
                error: None,
            })
        }
        "contents" => {
            let path = request_path(request.identifier.as_deref())?;
            let filename = Uuid::new_v4().to_string();
            let destination = payload_path(root, &filename)?;
            let mut output = fs::File::create(&destination)
                .await
                .map_err(StorageError::Io)?;
            let mut stream = provider
                .read(ReadRequest {
                    path: path.clone(),
                    range: None,
                })
                .await?;
            while let Some(chunk) = stream.next().await {
                output.write_all(&chunk?).await.map_err(StorageError::Io)?;
            }
            output.flush().await.map_err(StorageError::Io)?;
            let metadata = provider.stat(&path).await?;
            let mut response = item_response(remote_item(
                &connection,
                metadata,
                &provider.capabilities_for_path(&path),
            ));
            response.content_file = Some(filename);
            Ok(response)
        }
        "create" => {
            let parent = request_path(request.parent_identifier.as_deref())?;
            let filename = required_filename(request.filename.as_deref())?;
            let path = parent.join(filename).map_err(message_error)?;
            if request.is_directory.unwrap_or(false) {
                provider.create_directory(&path).await?;
            } else {
                write_content(
                    root,
                    provider.as_ref(),
                    &path,
                    request.content_file.as_deref(),
                )
                .await?;
            }
            Ok(item_response(remote_item(
                &connection,
                provider.stat(&path).await?,
                &provider.capabilities_for_path(&path),
            )))
        }
        "modify" => {
            let original = request_path(request.identifier.as_deref())?;
            let parent = request_path(request.parent_identifier.as_deref())?;
            let filename = required_filename(request.filename.as_deref())?;
            let target = parent.join(filename).map_err(message_error)?;
            if target != original {
                provider.rename(&original, &target).await?;
            }
            if request.content_file.is_some() {
                write_content(
                    root,
                    provider.as_ref(),
                    &target,
                    request.content_file.as_deref(),
                )
                .await?;
            }
            Ok(item_response(remote_item(
                &connection,
                provider.stat(&target).await?,
                &provider.capabilities_for_path(&target),
            )))
        }
        "delete" => {
            let path = request_path(request.identifier.as_deref())?;
            provider.delete(&path).await?;
            Ok(BrokerResponse {
                ok: true,
                item: None,
                items: None,
                next_page_token: None,
                content_file: None,
                error: None,
            })
        }
        _ => Err(StorageError::Unsupported {
            provider: provider.kind(),
            capability: request.operation.clone(),
        }
        .into()),
    }
}

async fn write_content(
    root: &Path,
    provider: &dyn bifrost_storage::StorageProvider,
    path: &RemotePath,
    content_file: Option<&str>,
) -> Result<(), BrokerFailure> {
    let content = if let Some(content_file) = content_file {
        let source = payload_path(root, content_file)?;
        let file = fs::File::open(&source).await.map_err(StorageError::Io)?;
        let stream = ReaderStream::new(file).map(|result| result.map_err(StorageError::Io));
        provider
            .write(WriteRequest {
                path: path.clone(),
                content: Box::pin(stream),
                size_bytes: fs::metadata(&source).await.ok().map(|value| value.len()),
                modified_at: None,
            })
            .await?;
        let _ = fs::remove_file(source).await;
        return Ok(());
    } else {
        futures_util::stream::empty()
    };
    Ok(provider
        .write(WriteRequest {
            path: path.clone(),
            content: Box::pin(content),
            size_bytes: Some(0),
            modified_at: None,
        })
        .await
        .map(|_| ())?)
}

fn request_path(value: Option<&str>) -> Result<RemotePath, BrokerFailure> {
    match value.unwrap_or_default() {
        "" => Ok(RemotePath::root()),
        value => RemotePath::parse(value).map_err(message_error),
    }
}

fn required_filename(value: Option<&str>) -> Result<&str, BrokerFailure> {
    let value = value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| message_error("filename is required"))?;
    if value.contains(['/', '\\']) || matches!(value, "." | "..") {
        return Err(message_error("filename is invalid"));
    }
    Ok(value)
}

fn payload_path(root: &Path, filename: &str) -> Result<PathBuf, BrokerFailure> {
    required_filename(Some(filename))?;
    Ok(root.join("payloads").join(filename))
}

fn root_metadata() -> RemoteMetadata {
    RemoteMetadata {
        path: RemotePath::root(),
        is_directory: true,
        size_bytes: None,
        etag: None,
        modified_at: None,
    }
}

fn remote_item(
    connection: &ConnectionRecord,
    metadata: RemoteMetadata,
    capabilities: &CapabilitySet,
) -> RemoteItem {
    let identifier = metadata.path.as_str().to_owned();
    let (parent_identifier, filename) = if identifier.is_empty() {
        (String::new(), connection.name.clone())
    } else if let Some((parent, filename)) = identifier.rsplit_once('/') {
        (parent.to_owned(), filename.to_owned())
    } else {
        (String::new(), identifier.clone())
    };
    RemoteItem {
        identifier,
        parent_identifier,
        filename,
        is_directory: metadata.is_directory,
        size: metadata
            .size_bytes
            .and_then(|value| i64::try_from(value).ok()),
        modified_at: metadata.modified_at.map(|value| value.to_rfc3339()),
        capabilities: capabilities
            .iter()
            .map(|capability| match capability {
                Capability::Read => "read",
                Capability::Write => "write",
                Capability::Delete => "delete",
                Capability::Rename | Capability::ServerSideMove => "rename",
                Capability::CreateDirectory => "create_directory",
                _ => "other",
            })
            .filter(|capability| *capability != "other")
            .collect(),
    }
}

fn item_response(item: RemoteItem) -> BrokerResponse {
    BrokerResponse {
        ok: true,
        item: Some(item),
        items: None,
        next_page_token: None,
        content_file: None,
        error: None,
    }
}

fn message_error(error: impl std::fmt::Display) -> BrokerFailure {
    BrokerFailure::Message(error.to_string())
}

fn error_code(error: &BrokerFailure) -> &'static str {
    let BrokerFailure::Storage(error) = error else {
        return "invalid_request";
    };
    match error {
        StorageError::AuthenticationFailed { .. } => "not_authenticated",
        StorageError::NotFound { .. } => "not_found",
        StorageError::PermissionDenied { .. } => "permission_denied",
        StorageError::Unsupported { .. } => "unsupported",
        StorageError::Cancelled => "cancelled",
        StorageError::Network { .. } | StorageError::Io(_) => "server_unreachable",
        StorageError::Provider { .. } => "provider_error",
    }
}

#[cfg(test)]
mod tests {
    use super::{mount_on_startup, payload_path, remote_item, request_path};
    use bifrost_common::{
        Capability, CapabilitySet, ConnectionId, ProviderKind, RemoteMetadata, RemotePath,
    };
    use bifrost_db::ConnectionRecord;
    use std::path::Path;

    #[test]
    fn maps_root_and_child_identifiers() {
        let connection = ConnectionRecord {
            id: ConnectionId::new(),
            name: "Team Files".to_owned(),
            kind: ProviderKind::WebDav,
            endpoint: "https://example.test".to_owned(),
            credential_ref: "{}".to_owned(),
            configuration_json: "{}".to_owned(),
        };
        let root = remote_item(
            &connection,
            RemoteMetadata {
                path: RemotePath::root(),
                is_directory: true,
                size_bytes: None,
                etag: None,
                modified_at: None,
            },
            &CapabilitySet::with([Capability::Read]),
        );
        assert_eq!(root.identifier, "");
        assert_eq!(root.filename, "Team Files");

        let child = remote_item(
            &connection,
            RemoteMetadata {
                path: RemotePath::parse("docs/report.txt").unwrap(),
                is_directory: false,
                size_bytes: Some(12),
                etag: None,
                modified_at: None,
            },
            &CapabilitySet::with([Capability::Read, Capability::Write]),
        );
        assert_eq!(child.parent_identifier, "docs");
        assert_eq!(child.filename, "report.txt");
        assert_eq!(child.capabilities, vec!["read", "write"]);
    }

    #[test]
    fn confines_payloads_to_the_shared_payload_directory() {
        assert!(payload_path(Path::new("/shared"), "../secret").is_err());
        assert!(request_path(Some("../secret")).is_err());
    }

    #[test]
    fn reads_startup_domain_preference() {
        assert!(mount_on_startup("{}"));
        assert!(mount_on_startup(r#"{"mount_on_startup":true}"#));
        assert!(!mount_on_startup(r#"{"mount_on_startup":false}"#));
    }
}
