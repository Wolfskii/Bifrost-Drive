use bifrost_common::{ConnectionId, ConnectionState, ProviderKind, RemotePath};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const API_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionSummary {
    pub id: ConnectionId,
    pub name: String,
    pub kind: ProviderKind,
    pub state: ConnectionState,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConnectionRequest {
    pub name: String,
    pub kind: ProviderKind,
    pub endpoint: String,
    pub credential_ref: String,
    pub configuration: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateS3ConnectionRequest {
    pub name: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub path_style: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebDavConnectionRequest {
    pub name: String,
    pub endpoint: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFtpConnectionRequest {
    pub name: String,
    pub endpoint: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSmbConnectionRequest {
    pub name: String,
    pub endpoint: String,
    pub username: String,
    pub password: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSftpConnectionRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub root_path: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub known_hosts: Option<String>,
    #[serde(default)]
    pub trust_on_first_use: bool,
    pub authentication: String,
    pub private_key_path: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionIdRequest {
    pub id: ConnectionId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionRequest {
    pub kind: ProviderKind,
    pub endpoint: String,
    pub credential_ref: String,
    pub configuration: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListFilesRequest {
    pub connection_id: ConnectionId,
    pub path: RemotePath,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydrateFileRequest {
    pub connection_id: ConnectionId,
    pub path: RemotePath,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydrateFileResponse {
    pub path: RemotePath,
    pub local_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReconcileRequest {
    pub base: Option<String>,
    pub local: Option<String>,
    pub remote: Option<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReconcileResponse {
    pub decision: String,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRunRequest {
    pub connection_id: ConnectionId,
    pub path: RemotePath,
    pub base: Option<String>,
    pub local: Option<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRunResponse {
    pub decision: String,
    pub conflict: bool,
    pub conflict_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRootRegisterRequest {
    pub connection_id: ConnectionId,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRootRegisterResponse {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveConflictRequest {
    pub id: Uuid,
    pub resolution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySummary {
    pub id: Uuid,
    pub kind: String,
    pub remote_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictSummary {
    pub id: Uuid,
    pub connection_id: ConnectionId,
    pub remote_path: String,
    pub local_fingerprint: Option<String>,
    pub remote_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSummary {
    pub path: RemotePath,
    pub is_directory: bool,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePage {
    pub entries: Vec<FileSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub id: Uuid,
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreS3CredentialRequest {
    pub label: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub api_version: u16,
    pub product_name: String,
    pub ready: bool,
}

impl Default for AppStatus {
    fn default() -> Self {
        Self {
            api_version: API_VERSION,
            product_name: "Bifrost Drive".to_owned(),
            ready: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
