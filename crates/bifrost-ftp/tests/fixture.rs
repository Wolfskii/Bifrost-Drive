use bifrost_common::RemotePath;
use bifrost_ftp::{FtpConfig, FtpProvider};
use bifrost_storage::{ReadRequest, StorageProvider, WriteRequest};
use bytes::Bytes;
use futures_util::{stream, TryStreamExt};
use std::{env, str::FromStr};
use url::Url;

fn integration_provider() -> Option<FtpProvider> {
    env::var_os("BIFROST_FTP_INTEGRATION")?;
    Some(
        FtpProvider::connect(FtpConfig {
            endpoint: Url::from_str(
                &env::var("BIFROST_FTP_ENDPOINT")
                    .unwrap_or_else(|_| "ftp://127.0.0.1:2121".to_owned()),
            )
            .unwrap(),
            username: env::var("BIFROST_FTP_USERNAME").unwrap_or_else(|_| "bifrost-dev".to_owned()),
            password: env::var("BIFROST_FTP_PASSWORD")
                .unwrap_or_else(|_| "bifrost-dev-secret".to_owned()),
        })
        .unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires an FTP/FTPS fixture and BIFROST_FTP_INTEGRATION=1"]
async fn ftp_round_trip_uses_streaming_io() {
    let Some(provider) = integration_provider() else {
        return;
    };
    provider.test_connection().await.unwrap();
    let path = RemotePath::parse("bifrost-round-trip.txt").unwrap();
    let payload = Bytes::from_static(b"Bifrost Drive FTP contract test");
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
