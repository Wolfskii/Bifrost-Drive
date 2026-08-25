use bifrost_common::RemotePath;
use bifrost_sftp::{SftpConfig, SftpProvider};
use bifrost_storage::{ReadRequest, StorageProvider, WriteRequest};
use bytes::Bytes;
use futures_util::{stream, TryStreamExt};
use std::{env, path::PathBuf};

fn integration_provider() -> Option<SftpProvider> {
    env::var_os("BIFROST_SFTP_INTEGRATION")?;
    let known_hosts = env::var("BIFROST_SFTP_KNOWN_HOSTS").unwrap_or_else(|_| {
        format!(
            "{}/../../.cache/sftp_known_hosts",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    Some(
        SftpProvider::connect(
            SftpConfig {
                host: "127.0.0.1".to_owned(),
                port: env::var("SFTP_PORT")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(2222),
                username: "bifrost-dev".to_owned(),
                known_hosts: PathBuf::from(known_hosts),
                root_path: String::new(),
                trust_on_first_use: false,
            },
            "bifrost-dev-secret",
        )
        .unwrap(),
    )
}

fn key_integration_provider() -> Option<SftpProvider> {
    env::var_os("BIFROST_SFTP_KEY_INTEGRATION")?;
    let known_hosts = env::var("BIFROST_SFTP_KNOWN_HOSTS").unwrap_or_else(|_| {
        format!(
            "{}/../../.cache/sftp_known_hosts",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let private_key = env::var("BIFROST_SFTP_PRIVATE_KEY").unwrap_or_else(|_| {
        format!(
            "{}/../../.cache/sftp_keys/id_ed25519",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    Some(
        SftpProvider::connect_with_private_key(
            SftpConfig {
                host: "127.0.0.1".to_owned(),
                port: env::var("SFTP_PORT")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(2222),
                username: "bifrost-dev".to_owned(),
                known_hosts: PathBuf::from(known_hosts),
                root_path: String::new(),
                trust_on_first_use: false,
            },
            PathBuf::from(private_key),
            None,
        )
        .unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires task docker:up and BIFROST_SFTP_INTEGRATION=1"]
async fn atmoz_sftp_round_trip_uses_known_hosts_and_streaming_io() {
    let Some(provider) = integration_provider() else {
        return;
    };
    provider.test_connection().await.unwrap();
    let path = RemotePath::parse("upload/round-trip.txt").unwrap();
    let renamed = RemotePath::parse("upload/renamed.txt").unwrap();
    let payload = Bytes::from_static(b"Bifrost Drive SFTP contract test");
    provider
        .write(WriteRequest {
            path: path.clone(),
            content: Box::pin(stream::iter(vec![Ok(payload.clone())])),
            size_bytes: Some(payload.len() as u64),
            modified_at: None,
        })
        .await
        .unwrap();
    let page = provider
        .list(&RemotePath::parse("upload").unwrap(), None)
        .await
        .unwrap();
    assert!(page.entries.iter().any(|entry| entry.metadata.path == path));
    provider.rename(&path, &renamed).await.unwrap();
    let mut content = provider
        .read(ReadRequest {
            path: renamed.clone(),
            range: Some(0..7),
        })
        .await
        .unwrap();
    assert_eq!(
        content.try_next().await.unwrap().unwrap(),
        Bytes::from_static(b"Bifrost")
    );
    provider.delete(&renamed).await.unwrap();
}

#[tokio::test]
#[ignore = "requires task docker:up, an ephemeral key, and BIFROST_SFTP_KEY_INTEGRATION=1"]
async fn atmoz_sftp_public_key_auth_uses_known_hosts() {
    let Some(provider) = key_integration_provider() else {
        return;
    };
    provider.test_connection().await.unwrap();
}
