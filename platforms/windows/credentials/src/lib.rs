use async_trait::async_trait;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
#[cfg(target_os = "windows")]
use uuid::Uuid;

#[cfg(target_os = "windows")]
const SERVICE_NAME: &str = "com.bifrost.drive";

pub struct WindowsCredentialStore;

impl WindowsCredentialStore {
    pub fn new() -> Self {
        Self
    }

    #[cfg(not(target_os = "windows"))]
    fn unavailable() -> CredentialError {
        CredentialError::Unavailable("Windows Credential Manager requires Windows".to_owned())
    }
}

impl Default for WindowsCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
#[async_trait]
impl CredentialStore for WindowsCredentialStore {
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
            let username = id.to_string();
            let entry = keyring::Entry::new(SERVICE_NAME, &username)
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
            let username = id.to_string();
            let entry = keyring::Entry::new(SERVICE_NAME, &username)
                .map_err(|error| CredentialError::Store(error.to_string()))?;
            let password = entry.get_password().map_err(|error| match error {
                keyring::Error::NoEntry => CredentialError::NotFound,
                other => CredentialError::Store(other.to_string()),
            })?;
            Ok(SecretString::new(password))
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }

    async fn delete(&self, credential: &CredentialRef) -> Result<(), CredentialError> {
        let id = credential.id;
        tokio::task::spawn_blocking(move || {
            let username = id.to_string();
            let entry = keyring::Entry::new(SERVICE_NAME, &username)
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

#[cfg(not(target_os = "windows"))]
#[async_trait]
impl CredentialStore for WindowsCredentialStore {
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

#[cfg(test)]
mod tests {
    use bifrost_crypto::SecretString;

    #[test]
    fn secret_debug_and_display_are_redacted() {
        let secret = SecretString::new("do-not-log");
        assert_eq!(format!("{secret:?}"), "SecretString(REDACTED)");
        assert_eq!(secret.to_string(), "[REDACTED]");
    }
}
