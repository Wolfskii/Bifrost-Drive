use bifrost_common::RemotePath;
use bifrost_smb::{SmbConfig, SmbProvider};
use bifrost_storage::{ReadRequest, StorageProvider, WriteRequest};
use bytes::Bytes;
use futures_util::{stream, TryStreamExt};
use std::{env, str::FromStr};
use url::Url;

async fn integration_provider() -> Option<SmbProvider> {
    env::var_os("BIFROST_SMB_INTEGRATION")?;
    let endpoint = Url::from_str(
        &env::var("BIFROST_SMB_ENDPOINT").unwrap_or_else(|_| "smb://127.0.0.1/share".to_owned()),
    )
    .unwrap();
    Some(
        SmbProvider::connect(SmbConfig {
            endpoint,
            username: env::var("BIFROST_SMB_USERNAME").unwrap_or_else(|_| "bifrost-dev".to_owned()),
            password: env::var("BIFROST_SMB_PASSWORD")
                .unwrap_or_else(|_| "bifrost-dev-secret".to_owned()),
            domain: env::var("BIFROST_SMB_DOMAIN").unwrap_or_default(),
        })
        .await
        .unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires an SMB2/3 fixture and BIFROST_SMB_INTEGRATION=1"]
async fn smb_round_trip_uses_streaming_io() {
    let Some(provider) = integration_provider().await else {
        return;
    };
    provider.test_connection().await.unwrap();
    let path = RemotePath::parse("bifrost-round-trip.txt").unwrap();
    let payload = Bytes::from_static(b"Bifrost Drive SMB contract test");
    provider
        .write(WriteRequest {
            path: path.clone(),
            content: Box::pin(stream::iter(vec![Ok(payload.clone())])),
            size_bytes: Some(payload.len() as u64),
            modified_at: None,
        })
        .await
        .unwrap();
    let content = provider
        .read(ReadRequest {
            path: path.clone(),
            range: None,
        })
        .await
        .unwrap();
    let received = content.try_collect::<Vec<_>>().await.unwrap();
    assert_eq!(received.concat(), payload);
    provider.delete(&path).await.unwrap();
}
