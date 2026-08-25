use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::{config::Builder as S3ConfigBuilder, primitives::ByteStream, Client};
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream as StorageByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider,
    WriteRequest,
};
use chrono::{DateTime, TimeZone, Utc};
use futures_util::StreamExt;
use http_body::Frame;
use http_body_util::StreamBody;
use std::ops::Range;
use tokio_util::io::ReaderStream;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Config {
    pub endpoint: Url,
    pub region: String,
    pub bucket: String,
    pub path_style: bool,
}

pub struct S3Provider {
    client: Client,
    bucket: String,
}

impl S3Provider {
    pub async fn connect(
        config: S3Config,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Result<Self, StorageError> {
        if config.bucket.trim().is_empty() {
            return Err(StorageError::Provider {
                provider: ProviderKind::S3,
                message: "bucket name must not be empty".to_owned(),
            });
        }

        let credentials = Credentials::new(
            access_key_id,
            secret_access_key,
            None,
            None,
            "bifrost-drive",
        );
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(config.region))
            .credentials_provider(credentials)
            .endpoint_url(config.endpoint.to_string())
            .load()
            .await;
        let service_config = S3ConfigBuilder::from(&shared_config)
            .force_path_style(config.path_style)
            .build();

        Ok(Self {
            client: Client::from_conf(service_config),
            bucket: config.bucket,
        })
    }

    fn object_key(path: &RemotePath) -> &str {
        path.as_str()
    }

    fn map_error<E: std::fmt::Display>(error: E) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::S3,
            message: error.to_string(),
        }
    }

    fn map_date(value: Option<&aws_smithy_types::DateTime>) -> Option<DateTime<Utc>> {
        value.and_then(|date| Utc.timestamp_opt(date.secs(), date.subsec_nanos()).single())
    }

    fn map_object(object: &aws_sdk_s3::types::Object) -> Result<Option<RemoteEntry>, StorageError> {
        let Some(key) = object.key() else {
            return Ok(None);
        };
        let path = RemotePath::parse(key).map_err(|error| StorageError::Provider {
            provider: ProviderKind::S3,
            message: error.to_string(),
        })?;
        Ok(Some(RemoteEntry {
            metadata: RemoteMetadata {
                path,
                is_directory: false,
                size_bytes: object.size().and_then(|size| u64::try_from(size).ok()),
                etag: object.e_tag().map(str::to_owned),
                modified_at: Self::map_date(object.last_modified()),
            },
        }))
    }

    fn range_header(range: Option<Range<u64>>) -> Result<Option<String>, StorageError> {
        let Some(range) = range else {
            return Ok(None);
        };
        if range.start >= range.end {
            return Err(StorageError::Provider {
                provider: ProviderKind::S3,
                message: "read range must have a positive length".to_owned(),
            });
        }
        Ok(Some(format!("bytes={}-{}", range.start, range.end - 1)))
    }

    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        if self
            .client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_ok()
        {
            return Ok(());
        }

        self.client
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map(|_| ())
            .map_err(Self::map_error)
    }
}

#[async_trait]
impl StorageProvider for S3Provider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::S3
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::RangeRead,
        ])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map(|_| ())
            .map_err(Self::map_error)
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let response = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(Self::object_key(prefix))
            .delimiter("/")
            .set_continuation_token(cursor.map(str::to_owned))
            .send()
            .await
            .map_err(Self::map_error)?;

        let mut entries = Vec::new();
        for object in response.contents() {
            if let Some(entry) = Self::map_object(object)? {
                entries.push(entry);
            }
        }
        for common_prefix in response.common_prefixes() {
            if let Some(prefix) = common_prefix.prefix() {
                let path = RemotePath::parse(prefix.trim_end_matches('/')).map_err(|error| {
                    StorageError::Provider {
                        provider: ProviderKind::S3,
                        message: error.to_string(),
                    }
                })?;
                entries.push(RemoteEntry {
                    metadata: RemoteMetadata {
                        path,
                        is_directory: true,
                        size_bytes: None,
                        etag: None,
                        modified_at: None,
                    },
                });
            }
        }

        Ok(Page {
            entries,
            next_cursor: response.next_continuation_token().map(str::to_owned),
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let response = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(Self::object_key(path))
            .send()
            .await
            .map_err(Self::map_error)?;
        Ok(RemoteMetadata {
            path: path.clone(),
            is_directory: false,
            size_bytes: response
                .content_length()
                .and_then(|size| u64::try_from(size).ok()),
            etag: response.e_tag().map(str::to_owned),
            modified_at: Self::map_date(response.last_modified()),
        })
    }

    async fn read(&self, request: ReadRequest) -> Result<StorageByteStream, StorageError> {
        let range = Self::range_header(request.range)?;
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::object_key(&request.path))
            .set_range(range)
            .send()
            .await
            .map_err(Self::map_error)?;
        let stream = ReaderStream::new(response.body.into_async_read())
            .map(|chunk| chunk.map_err(StorageError::Io));
        Ok(Box::pin(stream))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let path = request.path;
        let content_length = request.size_bytes;
        let body = StreamBody::new(request.content.map(|chunk| {
            chunk
                .map(Frame::data)
                .map_err(|error| std::io::Error::other(error.to_string()))
        }));
        let mut operation = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::object_key(&path))
            .body(ByteStream::from_body_1_x(body));
        if let Some(size) = content_length {
            operation = operation.content_length(i64::try_from(size).map_err(|_| {
                StorageError::Provider {
                    provider: ProviderKind::S3,
                    message: "object is too large for S3 content length".to_owned(),
                }
            })?);
        }
        let response = operation.send().await.map_err(Self::map_error)?;
        Ok(RemoteMetadata {
            path,
            is_directory: false,
            size_bytes: content_length,
            etag: response.e_tag().map(str::to_owned),
            modified_at: request.modified_at,
        })
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(Self::object_key(path))
            .send()
            .await
            .map(|_| ())
            .map_err(Self::map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::{S3Config, S3Provider};
    use bifrost_common::{Capability, CapabilitySet, RemotePath};
    use url::Url;

    #[test]
    fn config_preserves_s3_compatible_endpoint_options() {
        let config = S3Config {
            endpoint: Url::parse("http://127.0.0.1:9000").unwrap(),
            region: "us-east-1".to_owned(),
            bucket: "bifrost-integration".to_owned(),
            path_style: true,
        };

        assert_eq!(config.endpoint.as_str(), "http://127.0.0.1:9000/");
        assert_eq!(config.region, "us-east-1");
        assert!(config.path_style);
    }

    #[test]
    fn range_header_uses_exclusive_end_ranges() {
        assert_eq!(
            S3Provider::range_header(Some(2..7)).unwrap().as_deref(),
            Some("bytes=2-6")
        );
        assert!(S3Provider::range_header(Some(7..7)).is_err());
    }

    #[test]
    fn capability_set_contains_only_currently_implemented_operations() {
        let capabilities = CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::RangeRead,
        ]);
        assert!(capabilities.contains(Capability::RangeRead));
        assert!(!capabilities.contains(Capability::MultipartUpload));
        assert_eq!(RemotePath::root().as_str(), "");
    }
}
