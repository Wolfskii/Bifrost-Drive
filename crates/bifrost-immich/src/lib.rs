use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::{header, Client, Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::ops::Range;

pub const PHOTOS_DIRECTORY: &str = "Photos";
pub const ALBUMS_DIRECTORY: &str = "Albums";
const PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmichConfig {
    pub endpoint: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImmichCredentials {
    ApiKey(String),
    Password { email: String, password: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmichAuthConfiguration {
    pub authentication: String,
}

#[derive(Debug, Clone)]
enum ImmichSession {
    ApiKey(String),
    Bearer(String),
}

pub struct ImmichProvider {
    client: Client,
    endpoint: Url,
    session: ImmichSession,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct Asset {
    id: String,
    #[serde(rename = "originalFileName")]
    original_file_name: String,
    #[serde(rename = "originalFileSize")]
    original_file_size: Option<u64>,
    #[serde(rename = "fileCreatedAt")]
    file_created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct Album {
    id: String,
    #[serde(rename = "albumName")]
    album_name: String,
}

#[derive(Debug, Deserialize)]
struct AlbumDetails {
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    assets: Vec<Asset>,
    #[serde(rename = "nextPage")]
    next_page: Option<serde_json::Value>,
}

impl ImmichProvider {
    pub async fn connect_with_credentials(
        config: ImmichConfig,
        credentials: ImmichCredentials,
    ) -> Result<Self, StorageError> {
        let endpoint = normalize_endpoint(config.endpoint)?;
        if endpoint.host_str().is_none() {
            return Err(Self::provider_error("Immich endpoint must include a host"));
        }
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(Self::provider_error(
                "Immich endpoint must use HTTP or HTTPS",
            ));
        }
        let client = Client::builder().build().map_err(Self::network_error)?;
        let mut provider = Self {
            client,
            endpoint,
            session: ImmichSession::ApiKey(String::new()),
        };
        provider.session = match credentials {
            ImmichCredentials::ApiKey(api_key) if !api_key.trim().is_empty() => {
                ImmichSession::ApiKey(api_key)
            }
            ImmichCredentials::ApiKey(_) => {
                return Err(StorageError::AuthenticationFailed {
                    provider: ProviderKind::Immich,
                })
            }
            ImmichCredentials::Password { email, password }
                if !email.trim().is_empty() && !password.is_empty() =>
            {
                let response = provider
                    .client
                    .post(provider.api_url("auth/login"))
                    .json(&serde_json::json!({ "email": email, "password": password }))
                    .send()
                    .await
                    .map_err(Self::network_error)?;
                let response = provider.send(response).await?;
                let login = response
                    .json::<LoginResponse>()
                    .await
                    .map_err(Self::network_error)?;
                if login.access_token.trim().is_empty() {
                    return Err(StorageError::AuthenticationFailed {
                        provider: ProviderKind::Immich,
                    });
                }
                ImmichSession::Bearer(login.access_token)
            }
            ImmichCredentials::Password { .. } => {
                return Err(StorageError::AuthenticationFailed {
                    provider: ProviderKind::Immich,
                })
            }
        };
        Ok(provider)
    }

    pub async fn resolve_endpoint(
        input: &str,
        credentials: ImmichCredentials,
    ) -> Result<Url, StorageError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Self::provider_error("Immich server URL is required"));
        }
        if has_explicit_scheme(input) {
            let endpoint = Url::parse(input).map_err(|_| {
                Self::provider_error("Immich server URL must be a valid HTTP or HTTPS URL")
            })?;
            let provider =
                Self::connect_with_credentials(ImmichConfig { endpoint }, credentials).await?;
            provider.test_connection().await?;
            return Ok(provider.endpoint);
        }

        let mut failures = Vec::new();
        for scheme in ["https", "http"] {
            let endpoint = Url::parse(&format!("{scheme}://{input}"))
                .map_err(|_| Self::provider_error("Immich server URL is invalid"))?;
            match Self::connect_with_credentials(ImmichConfig { endpoint }, credentials.clone())
                .await
            {
                Ok(provider) => match provider.test_connection().await {
                    Ok(()) => return Ok(provider.endpoint),
                    Err(StorageError::AuthenticationFailed { .. }) => {
                        return Err(StorageError::AuthenticationFailed {
                            provider: ProviderKind::Immich,
                        })
                    }
                    Err(error) => failures.push(error.to_string()),
                },
                Err(StorageError::AuthenticationFailed { .. }) => {
                    return Err(StorageError::AuthenticationFailed {
                        provider: ProviderKind::Immich,
                    })
                }
                Err(error) => failures.push(error.to_string()),
            }
        }
        Err(Self::provider_error(format!(
            "could not connect using HTTPS or HTTP: {}",
            failures.join("; ")
        )))
    }

    fn provider_error(message: impl Into<String>) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::Immich,
            message: message.into(),
        }
    }

    fn network_error(error: reqwest::Error) -> StorageError {
        StorageError::Network {
            provider: ProviderKind::Immich,
            message: error.to_string(),
        }
    }

    fn api_url(&self, path: &str) -> Url {
        let mut url = self.endpoint.clone();
        let base = url.path().trim_end_matches('/');
        url.set_path(&format!("{base}/api/{path}"));
        url
    }

    fn authorized(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        let request = self.client.request(method, url);
        match &self.session {
            ImmichSession::ApiKey(api_key) => request.header("x-api-key", api_key),
            ImmichSession::Bearer(token) => request.bearer_auth(token),
        }
    }

    async fn send(&self, response: reqwest::Response) -> Result<reqwest::Response, StorageError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(StorageError::AuthenticationFailed {
                provider: ProviderKind::Immich,
            });
        }
        if status == StatusCode::NOT_FOUND {
            return Err(Self::provider_error("Immich API endpoint was not found"));
        }
        Err(Self::provider_error(format!(
            "server returned HTTP {status}"
        )))
    }

    async fn get_asset(&self, id: &str, _path: &RemotePath) -> Result<Asset, StorageError> {
        self.send(
            self.authorized(Method::GET, self.api_url(&format!("assets/{id}")))
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await?
        .json::<Asset>()
        .await
        .map_err(Self::network_error)
        .map_err(|error| Self::provider_error(format!("invalid Immich asset response: {error}")))
    }

    async fn get_album(&self, id: &str, _path: &RemotePath) -> Result<AlbumDetails, StorageError> {
        self.send(
            self.authorized(Method::GET, self.api_url(&format!("albums/{id}")))
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await?
        .json::<AlbumDetails>()
        .await
        .map_err(Self::network_error)
        .map_err(|error| Self::provider_error(format!("invalid Immich album response: {error}")))
    }

    async fn list_photos(&self, cursor: Option<&str>) -> Result<Page<Asset>, StorageError> {
        let page = cursor
            .unwrap_or("1")
            .parse::<u32>()
            .map_err(|_| Self::provider_error("Immich photo page cursor is invalid"))?;
        let response = self
            .send(
                self.authorized(Method::POST, self.api_url("search/metadata"))
                    .json(&serde_json::json!({ "page": page, "size": PAGE_SIZE }))
                    .send()
                    .await
                    .map_err(Self::network_error)?,
            )
            .await?
            .json::<SearchResponse>()
            .await
            .map_err(Self::network_error)?;
        Ok(Page {
            entries: response.assets,
            next_cursor: response.next_page.and_then(value_to_cursor),
        })
    }

    async fn list_albums(&self) -> Result<Vec<Album>, StorageError> {
        self.send(
            self.authorized(Method::GET, self.api_url("albums"))
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await?
        .json::<Vec<Album>>()
        .await
        .map_err(Self::network_error)
    }

    fn asset_metadata(asset: Asset, path: RemotePath) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: false,
            size_bytes: asset.original_file_size,
            etag: Some(asset.id),
            modified_at: asset.file_created_at,
        }
    }

    fn directory_metadata(path: RemotePath) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: true,
            size_bytes: None,
            etag: None,
            modified_at: None,
        }
    }

    fn asset_name(asset: &Asset) -> String {
        format!("{}--{}", encode_name(&asset.original_file_name), asset.id)
    }

    fn album_name(album: &Album) -> String {
        format!("{}--{}", encode_name(&album.album_name), album.id)
    }

    fn id_from_component(component: &str) -> Option<&str> {
        component.rsplit_once("--").map(|(_, id)| id)
    }

    fn range_header(range: Option<Range<u64>>) -> Result<Option<String>, StorageError> {
        match range {
            None => Ok(None),
            Some(range) if range.start < range.end => {
                Ok(Some(format!("bytes={}-{}", range.start, range.end - 1)))
            }
            Some(_) => Err(Self::provider_error(
                "read range must have a positive length",
            )),
        }
    }
}

#[async_trait]
impl StorageProvider for ImmichProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Immich
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([Capability::Read, Capability::RangeRead])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        self.send(
            self.authorized(Method::GET, self.api_url("server/ping"))
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await
        .map(|_| ())
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let components: Vec<_> = prefix
            .as_str()
            .split('/')
            .filter(|component| !component.is_empty())
            .collect();
        if components.is_empty() {
            return Ok(Page {
                entries: [PHOTOS_DIRECTORY, ALBUMS_DIRECTORY]
                    .into_iter()
                    .map(|name| RemoteEntry {
                        metadata: Self::directory_metadata(RemotePath::parse(name).unwrap()),
                    })
                    .collect(),
                next_cursor: None,
            });
        }
        if components.len() == 1 && components[0] == PHOTOS_DIRECTORY {
            let page = self.list_photos(cursor).await?;
            return Ok(Page {
                entries: page
                    .entries
                    .into_iter()
                    .map(|asset| {
                        let path = RemotePath::parse(PHOTOS_DIRECTORY)
                            .unwrap()
                            .join(&Self::asset_name(&asset))
                            .map_err(|error| Self::provider_error(error.to_string()))?;
                        Ok(RemoteEntry {
                            metadata: Self::asset_metadata(asset, path),
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?,
                next_cursor: page.next_cursor,
            });
        }
        if components.len() == 1 && components[0] == ALBUMS_DIRECTORY {
            let entries = self
                .list_albums()
                .await?
                .into_iter()
                .map(|album| {
                    let path = RemotePath::parse(ALBUMS_DIRECTORY)
                        .unwrap()
                        .join(&Self::album_name(&album))
                        .map_err(|error| Self::provider_error(error.to_string()))?;
                    Ok(RemoteEntry {
                        metadata: Self::directory_metadata(path),
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            return Ok(Page {
                entries,
                next_cursor: None,
            });
        }
        if components.len() == 2 && components[0] == ALBUMS_DIRECTORY {
            let album_id =
                Self::id_from_component(components[1]).ok_or_else(|| StorageError::NotFound {
                    path: prefix.clone(),
                })?;
            let page = cursor
                .unwrap_or("1")
                .parse::<u32>()
                .map_err(|_| Self::provider_error("Immich album page cursor is invalid"))?;
            let assets = self.get_album(album_id, prefix).await?.assets;
            let start = (page.saturating_sub(1) as usize) * PAGE_SIZE as usize;
            let entries = assets
                .into_iter()
                .skip(start)
                .take(PAGE_SIZE as usize)
                .map(|asset| {
                    let path = prefix
                        .join(&Self::asset_name(&asset))
                        .map_err(|error| Self::provider_error(error.to_string()))?;
                    Ok(RemoteEntry {
                        metadata: Self::asset_metadata(asset, path),
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            return Ok(Page {
                next_cursor: (!entries.is_empty() && entries.len() == PAGE_SIZE as usize)
                    .then(|| (page + 1).to_string()),
                entries,
            });
        }
        Err(StorageError::NotFound {
            path: prefix.clone(),
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let components: Vec<_> = path
            .as_str()
            .split('/')
            .filter(|component| !component.is_empty())
            .collect();
        if components.is_empty()
            || (components.len() == 1
                && matches!(components[0], PHOTOS_DIRECTORY | ALBUMS_DIRECTORY))
        {
            return Ok(Self::directory_metadata(path.clone()));
        }
        if components.len() == 2 && components[0] == PHOTOS_DIRECTORY {
            let id = Self::id_from_component(components[1])
                .ok_or_else(|| StorageError::NotFound { path: path.clone() })?;
            return Ok(Self::asset_metadata(
                self.get_asset(id, path).await?,
                path.clone(),
            ));
        }
        if components.len() == 2 && components[0] == ALBUMS_DIRECTORY {
            let id = Self::id_from_component(components[1])
                .ok_or_else(|| StorageError::NotFound { path: path.clone() })?;
            self.get_album(id, path).await?;
            return Ok(Self::directory_metadata(path.clone()));
        }
        if components.len() == 3 && components[0] == ALBUMS_DIRECTORY {
            let id = Self::id_from_component(components[2])
                .ok_or_else(|| StorageError::NotFound { path: path.clone() })?;
            return Ok(Self::asset_metadata(
                self.get_asset(id, path).await?,
                path.clone(),
            ));
        }
        Err(StorageError::NotFound { path: path.clone() })
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let range = Self::range_header(request.range)?;
        let components: Vec<_> = request
            .path
            .as_str()
            .split('/')
            .filter(|component| !component.is_empty())
            .collect();
        let id = match components.as_slice() {
            [root, component] if *root == PHOTOS_DIRECTORY => Self::id_from_component(component),
            [root, _, component] if *root == ALBUMS_DIRECTORY => Self::id_from_component(component),
            _ => None,
        }
        .ok_or_else(|| StorageError::Unsupported {
            provider: self.kind(),
            capability: "read_directory".to_owned(),
        })?;
        let mut request =
            self.authorized(Method::GET, self.api_url(&format!("assets/{id}/original")));
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        let response = self
            .send(request.send().await.map_err(Self::network_error)?)
            .await?;
        Ok(Box::pin(
            response
                .bytes_stream()
                .map(|chunk| chunk.map_err(Self::network_error)),
        ))
    }

    async fn write(&self, _request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "write".to_owned(),
        })
    }

    async fn delete(&self, _path: &RemotePath) -> Result<(), StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "delete".to_owned(),
        })
    }
}

fn normalize_endpoint(mut endpoint: Url) -> Result<Url, StorageError> {
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let path = endpoint.path().trim_end_matches('/').to_owned();
    endpoint.set_path(&path);
    Ok(endpoint)
}

fn has_explicit_scheme(value: &str) -> bool {
    value
        .split_once("://")
        .is_some_and(|(scheme, _)| !scheme.is_empty())
}

fn value_to_cursor(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) if value.is_empty() => None,
        serde_json::Value::String(value) => Some(value),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn encode_name(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('\\', "%5C")
}

#[cfg(test)]
mod tests {
    use super::{
        encode_name, has_explicit_scheme, normalize_endpoint, ImmichCredentials, ImmichProvider,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use url::Url;

    #[test]
    fn normalizes_trailing_slashes_and_proxy_paths() {
        let endpoint =
            normalize_endpoint(Url::parse("https://photos.example/immich///?x=1").unwrap())
                .unwrap();
        assert_eq!(endpoint.as_str(), "https://photos.example/immich");
    }

    #[test]
    fn detects_only_explicit_url_schemes() {
        assert!(has_explicit_scheme("https://photos.example"));
        assert!(has_explicit_scheme("http://localhost:2283"));
        assert!(!has_explicit_scheme("photos.example"));
    }

    #[test]
    fn escapes_asset_names() {
        assert_eq!(
            encode_name("holiday/100%\\original.jpg"),
            "holiday%2F100%25%5Coriginal.jpg"
        );
    }

    #[tokio::test]
    async fn scheme_less_endpoint_falls_back_to_http_and_sends_api_key() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let size = socket.read(&mut buffer).await.unwrap();
                if attempt == 1 {
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    assert!(request.contains("GET /api/server/ping HTTP/1.1"));
                    assert!(request.to_ascii_lowercase().contains("x-api-key: test-key"));
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                        )
                        .await
                        .unwrap();
                }
            }
        });

        let endpoint = ImmichProvider::resolve_endpoint(
            &format!("127.0.0.1:{port}"),
            ImmichCredentials::ApiKey("test-key".to_owned()),
        )
        .await
        .unwrap();

        assert_eq!(endpoint.scheme(), "http");
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        server.await.unwrap();
    }
}
