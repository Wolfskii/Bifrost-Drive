use bifrost_common::RemotePath;
use bifrost_storage::{ReadRequest, StorageProvider, WriteRequest};
use bytes::Bytes;
use futures_util::{stream, TryStreamExt};
use std::env;
use url::Url;

use bifrost_s3::{S3Config, S3Provider};

fn integration_provider() -> Option<(S3Config, String, String)> {
    env::var_os("BIFROST_S3_INTEGRATION")?;

    Some((
        S3Config {
            endpoint: Url::parse(
                &env::var("BIFROST_S3_ENDPOINT")
                    .unwrap_or_else(|_| "http://127.0.0.1:9000".to_owned()),
            )
            .unwrap(),
            region: env::var("BIFROST_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned()),
            bucket: env::var("BIFROST_S3_BUCKET")
                .unwrap_or_else(|_| "bifrost-integration".to_owned()),
            path_style: true,
        },
        env::var("MINIO_ROOT_USER").unwrap_or_else(|_| "bifrost-dev".to_owned()),
        env::var("MINIO_ROOT_PASSWORD").unwrap_or_else(|_| "bifrost-dev-secret".to_owned()),
    ))
}

#[tokio::test]
#[ignore = "requires an S3-compatible fixture and BIFROST_S3_INTEGRATION=1"]
async fn s3_compatible_round_trip_uses_the_provider_contract() {
    let Some((config, access_key, secret_key)) = integration_provider() else {
        return;
    };
    let provider = S3Provider::connect(config, access_key, secret_key)
        .await
        .unwrap();
    provider.ensure_bucket().await.unwrap();
    provider.test_connection().await.unwrap();

    let path = RemotePath::parse("contract/round-trip.txt").unwrap();
    let payload = Bytes::from_static(b"Bifrost Drive S3 contract test");
    provider
        .write(WriteRequest {
            path: path.clone(),
            content: Box::pin(stream::iter(vec![Ok(payload.clone())])),
            size_bytes: Some(payload.len() as u64),
            modified_at: None,
        })
        .await
        .unwrap();

    let page = provider.list(&RemotePath::root(), None).await.unwrap();
    assert!(page.entries.iter().any(|entry| entry.metadata.is_directory));

    let mut content = provider
        .read(ReadRequest {
            path: path.clone(),
            range: Some(0..7),
        })
        .await
        .unwrap();
    let downloaded = content.try_next().await.unwrap().unwrap();
    assert_eq!(downloaded, Bytes::from_static(b"Bifrost"));

    let stat = provider.stat(&path).await.unwrap();
    assert_eq!(stat.size_bytes, Some(payload.len() as u64));
    provider.delete(&path).await.unwrap();
}
