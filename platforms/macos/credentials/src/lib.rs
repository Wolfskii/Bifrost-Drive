use async_trait::async_trait;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
#[cfg(target_os = "macos")]
use uuid::Uuid;

#[cfg(target_os = "macos")]
const SERVICE_NAME: &str = "com.bifrost.drive";

pub struct MacosCredentialStore;

impl MacosCredentialStore {
    pub fn new() -> Self {
        Self
    }

    #[cfg(not(target_os = "macos"))]
    fn unavailable() -> CredentialError {
        CredentialError::Unavailable("macOS Keychain requires macOS".to_owned())
    }
}

impl Default for MacosCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
#[async_trait]
impl CredentialStore for MacosCredentialStore {
    async fn put(
        &self,
        kind: &str,
        label: &str,
        secret: SecretString,
    ) -> Result<CredentialRef, CredentialError> {
        let kind = kind.to_owned();
        let label = label.to_owned();
        tokio::task::spawn_blocking(move || {
            let id = Uuid::new_v4();
            let entry = keyring::Entry::new(SERVICE_NAME, &id.to_string())
                .map_err(|error| CredentialError::Store(error.to_string()))?;
            entry
                .set_password(secret.expose())
                .map_err(|error| CredentialError::Store(error.to_string()))?;
            Ok(CredentialRef { id, kind, label })
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }

    async fn get(&self, credential: &CredentialRef) -> Result<SecretString, CredentialError> {
        let id = credential.id;
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(SERVICE_NAME, &id.to_string())
                .map_err(|error| CredentialError::Store(error.to_string()))?;
            entry
                .get_password()
                .map(SecretString::new)
                .map_err(|error| match error {
                    keyring::Error::NoEntry => CredentialError::NotFound,
                    other => CredentialError::Store(other.to_string()),
                })
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }

    async fn delete(&self, credential: &CredentialRef) -> Result<(), CredentialError> {
        let id = credential.id;
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(SERVICE_NAME, &id.to_string())
                .map_err(|error| CredentialError::Store(error.to_string()))?;
            entry.delete_credential().map_err(|error| match error {
                keyring::Error::NoEntry => CredentialError::NotFound,
                other => CredentialError::Store(other.to_string()),
            })
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }
}

#[cfg(not(target_os = "macos"))]
#[async_trait]
impl CredentialStore for MacosCredentialStore {
    async fn put(
        &self,
        _kind: &str,
        _label: &str,
        _secret: SecretString,
    ) -> Result<CredentialRef, CredentialError> {
        Err(Self::unavailable())
    }

    async fn get(&self, _credential: &CredentialRef) -> Result<SecretString, CredentialError> {
        Err(Self::unavailable())
    }

    async fn delete(&self, _credential: &CredentialRef) -> Result<(), CredentialError> {
        Err(Self::unavailable())
    }
}
