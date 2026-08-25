use bifrost_common::{ConnectionId, ConnectionState, ProviderKind};
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
pub struct ConnectionIdRequest {
    pub id: ConnectionId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreCredentialRequest {
    pub kind: String,
    pub label: String,
    pub secret: String,
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
