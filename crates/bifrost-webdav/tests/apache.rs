use bifrost_common::RemotePath;
use bifrost_storage::{ReadRequest, StorageProvider, WriteRequest};
use bytes::Bytes;
use futures_util::{stream, TryStreamExt};
use std::env;
use url::Url;

use bifrost_webdav::{WebDavConfig, WebDavProvider};

fn integration_provider() -> Option<WebDavProvider> {
    env::var_os("BIFROST_WEBDAV_INTEGRATION")?;
    let endpoint =
        env::var("BIFROST_WEBDAV_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:8080/".to_owned());
    Some(
        WebDavProvider::connect(
            WebDavConfig {
                endpoint: Url::parse(&endpoint).unwrap(),
                username: env::var("WEBDAV_USERNAME").unwrap_or_else(|_| "bifrost-dev".to_owned()),
            },
            env::var("WEBDAV_PASSWORD").unwrap_or_else(|_| "bifrost-dev-secret".to_owned()),
        )
        .unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires task docker:up and BIFROST_WEBDAV_INTEGRATION=1"]
async fn webdav_round_trip_uses_real_dav_verbs() {
    let Some(provider) = integration_provider() else {
        return;
    };
    provider.test_connection().await.unwrap();

    let original = RemotePath::parse("round-trip.txt").unwrap();
    let renamed = RemotePath::parse("renamed.txt").unwrap();
    let payload = Bytes::from_static(b"Bifrost Drive WebDAV contract test");
    provider
        .write(WriteRequest {
            path: original.clone(),
            content: Box::pin(stream::iter(vec![Ok(payload.clone())])),
            size_bytes: Some(payload.len() as u64),
            modified_at: None,
        })
        .await
        .unwrap();

    let page = provider.list(&RemotePath::root(), None).await.unwrap();
    assert!(page
        .entries
        .iter()
        .any(|entry| entry.metadata.path == original));
    provider.rename(&original, &renamed).await.unwrap();
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
