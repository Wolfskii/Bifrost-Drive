use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, LockToken, Page, ReadRequest, RemoteEntry, StorageCapacity, StorageError,
    StorageProvider, WriteRequest,
};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use quick_xml::{events::Event, Reader};
use reqwest::{header, Client, Method, StatusCode, Url};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavConfig {
    pub endpoint: Url,
    pub username: String,
}

pub struct WebDavProvider {
    client: Client,
    endpoint: Url,
    username: String,
    password: String,
}

impl WebDavProvider {
    pub fn connect(
        config: WebDavConfig,
        password: impl Into<String>,
    ) -> Result<Self, StorageError> {
        if !matches!(config.endpoint.scheme(), "http" | "https") {
            return Err(StorageError::Provider {
                provider: ProviderKind::WebDav,
                message: "WebDAV endpoint must use HTTP or HTTPS".to_owned(),
            });
        }
        let client = Client::builder()
            .build()
            .map_err(|error| StorageError::Provider {
                provider: ProviderKind::WebDav,
                message: error.to_string(),
            })?;
        Ok(Self {
            client,
            endpoint: config.endpoint,
            username: config.username,
            password: password.into(),
        })
    }

    fn url_for(&self, path: &RemotePath) -> Result<Url, StorageError> {
        let mut url = self.endpoint.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| StorageError::Provider {
                    provider: ProviderKind::WebDav,
                    message: "WebDAV endpoint cannot be used as a base path".to_owned(),
                })?;
            segments.pop_if_empty();
            segments.extend(
                path.as_str()
                    .split('/')
                    .filter(|segment| !segment.is_empty()),
            );
        }
        Ok(url)
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .basic_auth(&self.username, Some(&self.password))
    }

    async fn send(&self, response: reqwest::Response) -> Result<reqwest::Response, StorageError> {
        let status = response.status();
        if status.is_success() || status == StatusCode::MULTI_STATUS {
            return Ok(response);
        }
        let error = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                StorageError::AuthenticationFailed {
                    provider: ProviderKind::WebDav,
                }
            }
            StatusCode::NOT_FOUND => StorageError::Provider {
                provider: ProviderKind::WebDav,
                message: "remote item was not found".to_owned(),
            },
            _ => StorageError::Provider {
                provider: ProviderKind::WebDav,
                message: format!("server returned HTTP {status}"),
            },
        };
        Err(error)
    }

    fn range_header(range: Option<Range<u64>>) -> Result<Option<String>, StorageError> {
        let Some(range) = range else {
            return Ok(None);
        };
        if range.start >= range.end {
            return Err(StorageError::Provider {
                provider: ProviderKind::WebDav,
                message: "read range must have a positive length".to_owned(),
            });
        }
        Ok(Some(format!("bytes={}-{}", range.start, range.end - 1)))
    }

    fn parse_multistatus(xml: &[u8], endpoint: &Url) -> Result<Vec<RemoteEntry>, StorageError> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);
        let mut buffer = Vec::new();
        let mut entries = Vec::new();
        let mut current: Option<ParsedEntry> = None;
        let mut field: Option<Vec<u8>> = None;

        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(event)) => {
                    let name = event.local_name().as_ref().to_vec();
                    if name == b"response" {
                        current = Some(ParsedEntry::default());
                    } else if current.is_some() {
                        field = Some(name);
                    }
                }
                Ok(Event::Empty(event)) => {
                    if let Some(entry) = current.as_mut() {
                        if event.local_name().as_ref() == b"collection" {
                            entry.is_directory = true;
                        }
                    }
                }
                Ok(Event::Text(text)) => {
                    if let (Some(entry), Some(field_name)) = (current.as_mut(), field.as_deref()) {
                        let value = text
                            .decode()
                            .map_err(|error| StorageError::Provider {
                                provider: ProviderKind::WebDav,
                                message: error.to_string(),
                            })?
                            .into_owned();
                        entry.set(field_name, value);
                    }
                }
                Ok(Event::End(event)) => {
                    let name = event.local_name();
                    if name.as_ref() == b"response" {
                        if let Some(parsed) = current.take() {
                            if let Some(entry) = parsed.finish(endpoint)? {
                                entries.push(entry);
                            }
                        }
                    } else if field.as_deref() == Some(name.as_ref()) {
                        field = None;
                    }
                }
                Ok(Event::Eof) => break,
                Err(error) => {
                    return Err(StorageError::Provider {
                        provider: ProviderKind::WebDav,
                        message: error.to_string(),
                    })
                }
                _ => {}
            }
            buffer.clear();
        }
        Ok(entries)
    }

    fn parse_capacity(xml: &[u8]) -> Result<Option<StorageCapacity>, StorageError> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);
        let mut buffer = Vec::new();
        let mut field: Option<Vec<u8>> = None;
        let mut used_bytes = None;
        let mut available_bytes = None;

        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(event)) => {
                    let name = event.local_name().as_ref().to_vec();
                    if matches!(
                        name.as_slice(),
                        b"quota-used-bytes" | b"quota-available-bytes"
                    ) {
                        field = Some(name);
                    }
                }
                Ok(Event::Text(text)) => {
                    if let Some(field_name) = field.as_deref() {
                        let value = text.decode().map_err(|error| StorageError::Provider {
                            provider: ProviderKind::WebDav,
                            message: error.to_string(),
                        })?;
                        let value =
                            value
                                .parse::<u64>()
                                .map_err(|error| StorageError::Provider {
                                    provider: ProviderKind::WebDav,
                                    message: format!("invalid WebDAV quota value: {error}"),
                                })?;
                        match field_name {
                            b"quota-used-bytes" => used_bytes = Some(value),
                            b"quota-available-bytes" => available_bytes = Some(value),
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(event)) => {
                    if field.as_deref() == Some(event.local_name().as_ref()) {
                        field = None;
                    }
                }
                Ok(Event::Eof) => break,
                Err(error) => {
                    return Err(StorageError::Provider {
                        provider: ProviderKind::WebDav,
                        message: error.to_string(),
                    })
                }
                _ => {}
            }
            buffer.clear();
        }

        Ok(used_bytes
            .zip(available_bytes)
            .map(|(used, available)| StorageCapacity {
                total_bytes: used.saturating_add(available),
                available_bytes: available,
            }))
    }
}

#[derive(Default)]
struct ParsedEntry {
    href: Option<String>,
    display_name: Option<String>,
    size_bytes: Option<u64>,
    modified_at: Option<DateTime<Utc>>,
    etag: Option<String>,
    is_directory: bool,
}

impl ParsedEntry {
    fn set(&mut self, field: &[u8], value: String) {
        match field {
            b"href" => self.href = Some(value),
            b"displayname" => self.display_name = Some(value),
            b"getcontentlength" => self.size_bytes = value.parse().ok(),
            b"getlastmodified" => {
                self.modified_at = DateTime::parse_from_rfc2822(&value)
                    .ok()
                    .map(|date| date.with_timezone(&Utc))
            }
            b"getetag" => self.etag = Some(value),
            _ => {}
        }
    }

    fn finish(self, endpoint: &Url) -> Result<Option<RemoteEntry>, StorageError> {
        let Some(href) = self.href else {
            return Ok(None);
        };
        let path = Url::parse(&href)
            .ok()
            .map(|url| url.path().to_owned())
            .unwrap_or(href);
        let base = endpoint.path().trim_end_matches('/');
        let relative = path
            .strip_prefix(base)
            .unwrap_or(path.as_str())
            .trim_matches('/');
        if relative.is_empty() {
            return Ok(None);
        }
        let remote_path = RemotePath::parse(relative).map_err(|error| StorageError::Provider {
            provider: ProviderKind::WebDav,
            message: error.to_string(),
        })?;
        Ok(Some(RemoteEntry {
            metadata: RemoteMetadata {
                path: remote_path,
                is_directory: self.is_directory,
                size_bytes: self.size_bytes,
                etag: self.etag,
                modified_at: self.modified_at,
            },
        }))
    }
}

#[async_trait]
impl StorageProvider for WebDavProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::WebDav
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::Rename,
            Capability::ServerSideCopy,
            Capability::CreateDirectory,
            Capability::Locking,
            Capability::RangeRead,
        ])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        self.send(
            self.request(Method::OPTIONS, self.endpoint.clone())
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
        )
        .await
        .map(|_| ())
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        _cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let body = br#"<?xml version="1.0" encoding="utf-8" ?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:getetag/><d:displayname/></d:prop></d:propfind>"#;
        let response = self
            .send(
                self.request(
                    Method::from_bytes(b"PROPFIND").unwrap(),
                    self.url_for(prefix)?,
                )
                .header("Depth", "1")
                .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
                .body(body.as_slice())
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
            )
            .await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|error| StorageError::Network {
                provider: ProviderKind::WebDav,
                message: error.to_string(),
            })?;
        Ok(Page {
            entries: Self::parse_multistatus(&bytes, &self.endpoint)?,
            next_cursor: None,
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        if path == &RemotePath::root() {
            return Ok(RemoteMetadata {
                path: path.clone(),
                is_directory: true,
                size_bytes: None,
                etag: None,
                modified_at: None,
            });
        }
        let page = self.list(path, None).await?;
        page.entries
            .into_iter()
            .find(|entry| entry.metadata.path == *path)
            .map(|entry| entry.metadata)
            .ok_or_else(|| StorageError::NotFound { path: path.clone() })
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let mut request_builder = self.request(Method::GET, self.url_for(&request.path)?);
        if let Some(range) = Self::range_header(request.range)? {
            request_builder = request_builder.header(header::RANGE, range);
        }
        let response = request_builder
            .send()
            .await
            .map_err(|error| StorageError::Network {
                provider: ProviderKind::WebDav,
                message: error.to_string(),
            })?;
        let response = self.send(response).await?;
        Ok(Box::pin(response.bytes_stream().map_err(|error| {
            StorageError::Network {
                provider: ProviderKind::WebDav,
                message: error.to_string(),
            }
        })))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let path = request.path;
        let response = self
            .request(Method::PUT, self.url_for(&path)?)
            .body(reqwest::Body::wrap_stream(
                request.content.map_ok(|bytes| bytes),
            ))
            .send()
            .await
            .map_err(|error| StorageError::Network {
                provider: ProviderKind::WebDav,
                message: error.to_string(),
            })?;
        self.send(response).await?;
        Ok(RemoteMetadata {
            path,
            is_directory: false,
            size_bytes: request.size_bytes,
            etag: None,
            modified_at: request.modified_at,
        })
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        self.send(
            self.request(Method::DELETE, self.url_for(path)?)
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
        )
        .await
        .map(|_| ())
    }

    async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
        let body = br#"<?xml version="1.0" encoding="utf-8" ?><d:propfind xmlns:d="DAV:"><d:prop><d:quota-used-bytes/><d:quota-available-bytes/></d:prop></d:propfind>"#;
        let response = self
            .send(
                self.request(
                    Method::from_bytes(b"PROPFIND").unwrap(),
                    self.endpoint.clone(),
                )
                .header("Depth", "0")
                .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
                .body(body.as_slice())
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
            )
            .await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|error| StorageError::Network {
                provider: ProviderKind::WebDav,
                message: error.to_string(),
            })?;
        Self::parse_capacity(&bytes)
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        self.send(
            self.request(Method::from_bytes(b"MKCOL").unwrap(), self.url_for(path)?)
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
        )
        .await
        .map(|_| ())
    }

    async fn copy(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let destination = self.url_for(to)?.to_string();
        self.send(
            self.request(Method::from_bytes(b"COPY").unwrap(), self.url_for(from)?)
                .header("Destination", destination)
                .header("Overwrite", "F")
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
        )
        .await
        .map(|_| ())
    }

    async fn lock(
        &self,
        path: &RemotePath,
        owner: &str,
        timeout_seconds: u64,
    ) -> Result<LockToken, StorageError> {
        let timeout = format!("Second-{}", timeout_seconds.max(1));
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8" ?><d:lockinfo xmlns:d="DAV:"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype><d:owner><d:href>{owner}</d:href></d:owner></d:lockinfo>"#
        );
        let response = self
            .send(
                self.request(Method::from_bytes(b"LOCK").unwrap(), self.url_for(path)?)
                    .header("Timeout", timeout)
                    .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
                    .body(body)
                    .send()
                    .await
                    .map_err(|error| StorageError::Network {
                        provider: ProviderKind::WebDav,
                        message: error.to_string(),
                    })?,
            )
            .await?;
        let token = response
            .headers()
            .get("Lock-Token")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| StorageError::Provider {
                provider: ProviderKind::WebDav,
                message: "WebDAV LOCK response did not include a Lock-Token".to_owned(),
            })?;
        Ok(LockToken { token })
    }

    async fn unlock(&self, path: &RemotePath, token: &LockToken) -> Result<(), StorageError> {
        self.send(
            self.request(Method::from_bytes(b"UNLOCK").unwrap(), self.url_for(path)?)
                .header("Lock-Token", &token.token)
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
        )
        .await
        .map(|_| ())
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let destination = self.url_for(to)?.to_string();
        self.send(
            self.request(Method::from_bytes(b"MOVE").unwrap(), self.url_for(from)?)
                .header("Destination", destination)
                .header("Overwrite", "F")
                .send()
                .await
                .map_err(|error| StorageError::Network {
                    provider: ProviderKind::WebDav,
                    message: error.to_string(),
                })?,
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{WebDavConfig, WebDavProvider};
    use bifrost_common::RemotePath;
    use bifrost_storage::StorageProvider;
    use url::Url;

    #[test]
    fn parses_dav_metadata_without_including_the_collection_root() {
        let xml = br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>https://dav.test/files/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>https://dav.test/files/report.txt</d:href><d:propstat><d:prop><d:getcontentlength>12</d:getcontentlength><d:getetag>\"abc\"</d:getetag></d:prop></d:propstat></d:response></d:multistatus>"#;
        let entries =
            WebDavProvider::parse_multistatus(xml, &Url::parse("https://dav.test/files/").unwrap())
                .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metadata.path.as_str(), "report.txt");
        assert_eq!(entries[0].metadata.size_bytes, Some(12));
    }

    #[test]
    fn parses_dav_quota_capacity() {
        let xml = br#"<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:quota-used-bytes>100</d:quota-used-bytes><d:quota-available-bytes>900</d:quota-available-bytes></d:prop></d:propstat></d:response></d:multistatus>"#;
        let capacity = WebDavProvider::parse_capacity(xml).unwrap().unwrap();
        assert_eq!(capacity.total_bytes, 1_000);
        assert_eq!(capacity.available_bytes, 900);
    }

    #[test]
    fn leaves_dav_capacity_unknown_without_both_quota_properties() {
        let xml = br#"<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:quota-available-bytes>900</d:quota-available-bytes></d:prop></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(WebDavProvider::parse_capacity(xml).unwrap(), None);
    }

    #[test]
    fn rejects_non_http_endpoints() {
        let result = WebDavProvider::connect(
            WebDavConfig {
                endpoint: Url::parse("sftp://dav.test").unwrap(),
                username: "user".to_owned(),
            },
            "secret",
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn configured_endpoint_is_the_remote_directory_root() {
        let provider = WebDavProvider::connect(
            WebDavConfig {
                endpoint: Url::parse("https://dav.test/files").unwrap(),
                username: "user".to_owned(),
            },
            "secret",
        )
        .unwrap();

        let metadata = provider.stat(&RemotePath::root()).await.unwrap();

        assert_eq!(metadata.path, RemotePath::root());
        assert!(metadata.is_directory);
        assert_eq!(metadata.size_bytes, None);
    }
}
