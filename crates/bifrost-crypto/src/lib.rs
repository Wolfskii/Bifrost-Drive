use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRef {
    pub id: Uuid,
    pub kind: String,
    pub label: String,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(REDACTED)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("native credential store is unavailable: {0}")]
    Unavailable(String),
    #[error("credential was not found")]
    NotFound,
    #[error("native credential store rejected the operation: {0}")]
    Store(String),
}

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn put(
        &self,
        kind: &str,
        label: &str,
        secret: SecretString,
    ) -> Result<CredentialRef, CredentialError>;
    async fn get(&self, credential: &CredentialRef) -> Result<SecretString, CredentialError>;
    async fn delete(&self, credential: &CredentialRef) -> Result<(), CredentialError>;
}
