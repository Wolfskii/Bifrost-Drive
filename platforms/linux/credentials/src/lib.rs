use async_trait::async_trait;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
#[cfg(target_os = "linux")]
use uuid::Uuid;

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "com.bifrost.drive";

#[cfg(target_os = "linux")]
fn map_keyring_error(error: keyring::Error) -> CredentialError {
    map_keyring_error_ref(&error)
}

#[cfg(target_os = "linux")]
fn map_keyring_error_ref(error: &keyring::Error) -> CredentialError {
    match error {
        keyring::Error::NoDefaultStore | keyring::Error::NoStorageAccess(_) => {
            CredentialError::Unavailable(
                "Linux Secret Service could not access an active, unlocked wallet".to_owned(),
            )
        }
        other => CredentialError::Store(other.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn entry(id: Uuid) -> Result<keyring::Entry, CredentialError> {
    keyring::Entry::new(SERVICE_NAME, &id.to_string()).map_err(map_keyring_error)
}

pub struct LinuxCredentialStore;

impl LinuxCredentialStore {
    pub fn new() -> Self {
        Self
    }

    #[cfg(target_os = "linux")]
    pub fn status() -> Result<(), CredentialError> {
        match keyring::Entry::store_status() {
            Ok(()) => Ok(()),
            Err(error) => Err(map_keyring_error_ref(error)),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn unavailable() -> CredentialError {
        CredentialError::Unavailable("Linux Secret Service requires Linux".to_owned())
    }
}

impl Default for LinuxCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl CredentialStore for LinuxCredentialStore {
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
            let entry = entry(id)?;
            entry
                .set_password(secret.expose())
                .map_err(map_keyring_error)?;
            Ok(CredentialRef { id, kind, label })
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }

    async fn get(&self, credential: &CredentialRef) -> Result<SecretString, CredentialError> {
        let id = credential.id;
        tokio::task::spawn_blocking(move || {
            let entry = entry(id)?;
            entry
                .get_password()
                .map(SecretString::new)
                .map_err(|error| match error {
                    keyring::Error::NoEntry => CredentialError::NotFound,
                    other => map_keyring_error(other),
                })
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }

    async fn delete(&self, credential: &CredentialRef) -> Result<(), CredentialError> {
        let id = credential.id;
        tokio::task::spawn_blocking(move || {
            let entry = entry(id)?;
            entry.delete_credential().map_err(|error| match error {
                keyring::Error::NoEntry => CredentialError::NotFound,
                other => map_keyring_error(other),
            })
        })
        .await
        .map_err(|error| CredentialError::Store(error.to_string()))?
    }
}

#[cfg(not(target_os = "linux"))]
#[async_trait]
impl CredentialStore for LinuxCredentialStore {
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
