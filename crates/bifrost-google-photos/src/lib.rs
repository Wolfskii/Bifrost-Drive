use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_google_drive::GoogleDriveProvider;
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageError, StorageProvider, WriteRequest,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::{header, Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::{ops::Range, sync::Arc};
use tokio::sync::Mutex;
use url::Url;

pub const GOOGLE_PHOTOS_ENDPOINT: &str = "https://photoslibrary.googleapis.com/v1";
pub const ALL_PHOTOS_DIRECTORY: &str = "All Photos";
pub const ALBUMS_DIRECTORY: &str = "Albums";
pub const LEGACY_DIRECTORY: &str = "Legacy";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GooglePhotosConfig {
    pub endpoint: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GooglePhotosCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub expires_at: Option<i64>,
}

struct GooglePhotosSession {
    access_token: String,
    refresh_token: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    expires_at: Option<i64>,
}

pub struct GooglePhotosProvider {
    client: Client,
    endpoint: Url,
    session: Arc<Mutex<GooglePhotosSession>>,
}

pub struct HybridGooglePhotosProvider {
    photos: GooglePhotosProvider,
    legacy: Option<GoogleDriveProvider>,
}

impl HybridGooglePhotosProvider {
    pub fn new(photos: GooglePhotosProvider, legacy: Option<GoogleDriveProvider>) -> Self {
        Self { photos, legacy }
    }

    fn legacy_path(path: &RemotePath) -> Result<RemotePath, StorageError> {
        let mut components = path.as_str().split('/');
        if components.next() != Some(LEGACY_DIRECTORY) {
            return Err(StorageError::NotFound { path: path.clone() });
        }
        RemotePath::parse(&components.collect::<Vec<_>>().join("/"))
            .map_err(|error| GooglePhotosProvider::provider_error(error.to_string()))
    }

    fn prefixed_legacy_metadata(metadata: RemoteMetadata) -> Result<RemoteMetadata, StorageError> {
        Ok(RemoteMetadata {
            path: RemotePath::parse(LEGACY_DIRECTORY)
                .unwrap()
                .join(metadata.path.as_str())
                .map_err(|error| GooglePhotosProvider::provider_error(error.to_string()))?,
            ..metadata
        })
    }

    fn is_legacy(path: &RemotePath) -> bool {
        path.as_str() == LEGACY_DIRECTORY || path.as_str().starts_with("Legacy/")
    }

    fn legacy_provider(&self, path: &RemotePath) -> Result<&GoogleDriveProvider, StorageError> {
        self.legacy
            .as_ref()
            .ok_or_else(|| StorageError::NotFound { path: path.clone() })
    }
}

#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct AlbumListResponse {
    #[serde(default)]
    albums: Vec<Album>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MediaListResponse {
    #[serde(default, rename = "mediaItems")]
    media_items: Vec<MediaItem>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Album {
    id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct MediaItem {
    id: String,
    filename: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "mediaMetadata")]
    media_metadata: Option<MediaMetadata>,
}

#[derive(Debug, Deserialize)]
struct MediaMetadata {
    #[serde(rename = "creationTime")]
    creation_time: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct CreateAlbumRequest<'a> {
    album: CreateAlbum<'a>,
}

#[derive(Serialize)]
struct CreateAlbum<'a> {
    title: &'a str,
}

#[derive(Serialize)]
struct BatchCreateRequest<'a> {
    #[serde(rename = "albumId", skip_serializing_if = "Option::is_none")]
    album_id: Option<&'a str>,
    #[serde(rename = "newMediaItems")]
    new_media_items: Vec<NewMediaItem<'a>>,
}

#[derive(Serialize)]
struct NewMediaItem<'a> {
    #[serde(rename = "simpleMediaItem")]
    simple_media_item: SimpleMediaItem<'a>,
}

#[derive(Serialize)]
struct SimpleMediaItem<'a> {
    #[serde(rename = "fileName")]
    file_name: &'a str,
    #[serde(rename = "uploadToken")]
    upload_token: &'a str,
}

#[derive(Deserialize)]
struct BatchCreateResponse {
    #[serde(rename = "newMediaItemResults")]
    new_media_item_results: Vec<NewMediaItemResult>,
}

#[derive(Deserialize)]
struct NewMediaItemResult {
    status: Option<GoogleStatus>,
    #[serde(rename = "mediaItem")]
    media_item: Option<MediaItem>,
}

#[derive(Deserialize)]
struct GoogleStatus {
    code: Option<i32>,
    message: Option<String>,
}

#[derive(Serialize)]
struct UpdateAlbumRequest<'a> {
    title: &'a str,
}

impl GooglePhotosProvider {
    pub fn connect_with_credentials(
        config: GooglePhotosConfig,
        credentials: GooglePhotosCredentials,
    ) -> Result<Self, StorageError> {
        if config.endpoint.scheme() != "https" {
            return Err(Self::provider_error(
                "Google Photos endpoint must use HTTPS",
            ));
        }
        if credentials.access_token.trim().is_empty() {
            return Err(StorageError::AuthenticationFailed {
                provider: ProviderKind::GooglePhotos,
            });
        }
        let client = Client::builder().build().map_err(Self::network_error)?;
        Ok(Self {
            client,
            endpoint: config.endpoint,
            session: Arc::new(Mutex::new(GooglePhotosSession {
                access_token: credentials.access_token,
                refresh_token: credentials.refresh_token,
                client_id: credentials.client_id,
                client_secret: credentials.client_secret,
                expires_at: credentials.expires_at,
            })),
        })
    }

    fn provider_error(message: impl Into<String>) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::GooglePhotos,
            message: message.into(),
        }
    }

    fn network_error(error: reqwest::Error) -> StorageError {
        StorageError::Network {
            provider: ProviderKind::GooglePhotos,
            message: error.to_string(),
        }
    }

    fn api_url(&self, path: &str) -> Url {
        let mut url = self.endpoint.clone();
        let base = url.path().trim_end_matches('/');
        url.set_path(&format!("{base}/{path}"));
        url
    }

    fn authorized(&self, method: Method, url: Url, access_token: &str) -> reqwest::RequestBuilder {
        self.client.request(method, url).bearer_auth(access_token)
    }

    async fn access_token(&self) -> Result<String, StorageError> {
        let mut session = self.session.lock().await;
        if session
            .expires_at
            .is_none_or(|expires_at| expires_at > Utc::now().timestamp() + 60)
        {
            return Ok(session.access_token.clone());
        }
        let (Some(refresh_token), Some(client_id), Some(client_secret)) = (
            session.refresh_token.clone(),
            session.client_id.clone(),
            session.client_secret.clone(),
        ) else {
            return Ok(session.access_token.clone());
        };
        let response = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(Self::network_error)?;
        if !response.status().is_success() {
            return Err(StorageError::AuthenticationFailed {
                provider: ProviderKind::GooglePhotos,
            });
        }
        let token = response
            .json::<RefreshTokenResponse>()
            .await
            .map_err(Self::network_error)?;
        session.access_token = token.access_token;
        session.expires_at = Some(Utc::now().timestamp() + token.expires_in);
        Ok(session.access_token.clone())
    }

    async fn send(&self, response: reqwest::Response) -> Result<reqwest::Response, StorageError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(StorageError::AuthenticationFailed {
                provider: ProviderKind::GooglePhotos,
            });
        }
        if status == StatusCode::NOT_FOUND {
            return Err(Self::provider_error("remote item was not found"));
        }
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| format!("server returned HTTP {status}"));
        Err(Self::provider_error(format!(
            "server returned HTTP {status}: {message}"
        )))
    }

    fn split_path(path: &RemotePath) -> (&str, Vec<&str>) {
        let mut components = path
            .as_str()
            .split('/')
            .filter(|component| !component.is_empty());
        let root = components.next().unwrap_or_default();
        (root, components.collect())
    }

    fn root_metadata(path: RemotePath) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: true,
            size_bytes: None,
            etag: None,
            modified_at: None,
        }
    }

    fn encode_name(value: &str) -> String {
        value
            .replace('%', "%25")
            .replace('/', "%2F")
            .replace('\\', "%5C")
    }

    fn item_path(prefix: &RemotePath, item: &MediaItem) -> Result<RemotePath, StorageError> {
        prefix
            .join(&format!(
                "{}--{}",
                Self::encode_name(&item.filename),
                item.id
            ))
            .map_err(|error| Self::provider_error(error.to_string()))
    }

    async fn list_media(
        &self,
        album_id: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page<MediaItem>, StorageError> {
        let access_token = self.access_token().await?;
        let response = if let Some(album_id) = album_id {
            self.authorized(
                Method::POST,
                self.api_url("mediaItems:search"),
                &access_token,
            )
            .json(&serde_json::json!({ "albumId": album_id, "pageSize": 100, "pageToken": cursor }))
            .send()
            .await
            .map_err(Self::network_error)?
        } else {
            let mut request = self
                .authorized(Method::GET, self.api_url("mediaItems"), &access_token)
                .query(&[("pageSize", "100")]);
            if let Some(cursor) = cursor {
                request = request.query(&[("pageToken", cursor)]);
            }
            request.send().await.map_err(Self::network_error)?
        };
        let page = self
            .send(response)
            .await?
            .json::<MediaListResponse>()
            .await
            .map_err(Self::network_error)?;
        Ok(Page {
            entries: page.media_items,
            next_cursor: page.next_page_token,
        })
    }

    async fn list_albums(&self, cursor: Option<&str>) -> Result<Page<Album>, StorageError> {
        let access_token = self.access_token().await?;
        let mut request = self
            .authorized(Method::GET, self.api_url("albums"), &access_token)
            .query(&[("pageSize", "50")]);
        if let Some(cursor) = cursor {
            request = request.query(&[("pageToken", cursor)]);
        }
        let page = self
            .send(request.send().await.map_err(Self::network_error)?)
            .await?
            .json::<AlbumListResponse>()
            .await
            .map_err(Self::network_error)?;
        Ok(Page {
            entries: page.albums,
            next_cursor: page.next_page_token,
        })
    }

    async fn find_album(&self, name: &str) -> Result<Album, StorageError> {
        let mut cursor = None;
        loop {
            let page = self.list_albums(cursor.as_deref()).await?;
            if let Some(album) = page.entries.into_iter().find(|album| album.title == name) {
                return Ok(album);
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Err(StorageError::NotFound {
                    path: RemotePath::parse(ALBUMS_DIRECTORY).unwrap(),
                });
            }
        }
    }

    async fn find_media(
        &self,
        album_id: Option<&str>,
        name: &str,
        path: &RemotePath,
    ) -> Result<MediaItem, StorageError> {
        let mut cursor = None;
        loop {
            let page = self.list_media(album_id, cursor.as_deref()).await?;
            if let Some(item) = page.entries.into_iter().find(|item| {
                Self::item_path(&RemotePath::root(), item)
                    .ok()
                    .as_ref()
                    .map(RemotePath::as_str)
                    == Some(name)
            }) {
                return Ok(item);
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Err(StorageError::NotFound { path: path.clone() });
            }
        }
    }

    async fn upload(
        &self,
        request: WriteRequest,
        album_id: Option<&str>,
    ) -> Result<RemoteMetadata, StorageError> {
        let path = request.path;
        let filename = path.as_str().rsplit('/').next().unwrap_or_default();
        if filename.is_empty() {
            return Err(Self::provider_error(
                "a file name is required for Google Photos uploads",
            ));
        }
        let mut content = request.content;
        let mut bytes = Vec::new();
        while let Some(chunk) = content.next().await {
            bytes.extend_from_slice(&chunk?);
        }
        let bytes = Bytes::from(bytes);
        let access_token = self.access_token().await?;
        let upload_token = self
            .send(
                self.authorized(Method::POST, self.api_url("uploads"), &access_token)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header("X-Goog-Upload-Content-Type", "application/octet-stream")
                    .header("X-Goog-Upload-Protocol", "raw")
                    .body(bytes)
                    .send()
                    .await
                    .map_err(Self::network_error)?,
            )
            .await?
            .text()
            .await
            .map_err(Self::network_error)?;
        let response = self
            .send(
                self.authorized(
                    Method::POST,
                    self.api_url("mediaItems:batchCreate"),
                    &access_token,
                )
                .json(&BatchCreateRequest {
                    album_id,
                    new_media_items: vec![NewMediaItem {
                        simple_media_item: SimpleMediaItem {
                            file_name: filename,
                            upload_token: &upload_token,
                        },
                    }],
                })
                .send()
                .await
                .map_err(Self::network_error)?,
            )
            .await?;
        let result = response
            .json::<BatchCreateResponse>()
            .await
            .map_err(Self::network_error)?
            .new_media_item_results
            .into_iter()
            .next()
            .ok_or_else(|| Self::provider_error("Google Photos did not return an upload result"))?;
        if let Some(status) = result
            .status
            .filter(|status| status.code.unwrap_or_default() != 0)
        {
            return Err(Self::provider_error(
                status
                    .message
                    .unwrap_or_else(|| "Google Photos rejected the upload".to_owned()),
            ));
        }
        let item = result
            .media_item
            .ok_or_else(|| Self::provider_error("Google Photos did not create a media item"))?;
        let parent = match album_id {
            Some(album_id) => RemotePath::parse(&format!("{ALBUMS_DIRECTORY}/{album_id}")).unwrap(),
            None => RemotePath::parse(ALL_PHOTOS_DIRECTORY).unwrap(),
        };
        let item_path = Self::item_path(&parent, &item)?;
        Ok(Self::media_metadata(item, item_path))
    }

    fn media_metadata(item: MediaItem, path: RemotePath) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: false,
            size_bytes: None,
            etag: Some(item.id),
            modified_at: item
                .media_metadata
                .and_then(|metadata| metadata.creation_time),
        }
    }

    fn range_suffix(range: Option<Range<u64>>) -> Result<String, StorageError> {
        match range {
            None => Ok("=d".to_owned()),
            Some(range) if range.start < range.end => {
                Ok(format!("=d-r{}-{}", range.start, range.end - 1))
            }
            Some(_) => Err(Self::provider_error(
                "read range must have a positive length",
            )),
        }
    }
}

#[async_trait]
impl StorageProvider for GooglePhotosProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GooglePhotos
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Rename,
            Capability::CreateDirectory,
            Capability::RangeRead,
        ])
    }

    fn capabilities_for_path(&self, path: &RemotePath) -> CapabilitySet {
        let (root, components) = Self::split_path(path);
        if root == ALBUMS_DIRECTORY && components.len() == 1 {
            return CapabilitySet::with([
                Capability::Read,
                Capability::Write,
                Capability::Rename,
                Capability::RangeRead,
            ]);
        }
        self.capabilities()
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        self.list(&RemotePath::root(), None).await.map(|_| ())
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let (root, components) = Self::split_path(prefix);
        if root.is_empty() {
            return Ok(Page {
                entries: [ALL_PHOTOS_DIRECTORY, ALBUMS_DIRECTORY]
                    .into_iter()
                    .map(|name| RemotePath::parse(name).unwrap())
                    .map(|path| RemoteEntry {
                        metadata: Self::root_metadata(path),
                    })
                    .collect(),
                next_cursor: None,
            });
        }
        if root == ALL_PHOTOS_DIRECTORY && components.is_empty() {
            let page = self.list_media(None, cursor).await?;
            return Ok(Page {
                entries: page
                    .entries
                    .into_iter()
                    .map(|item| {
                        let item_path = Self::item_path(prefix, &item)?;
                        Ok(RemoteEntry {
                            metadata: Self::media_metadata(item, item_path),
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?,
                next_cursor: page.next_cursor,
            });
        }
        if root == ALBUMS_DIRECTORY && components.is_empty() {
            let page = self.list_albums(cursor).await?;
            return Ok(Page {
                entries: page
                    .entries
                    .into_iter()
                    .map(|album| {
                        RemotePath::parse(ALBUMS_DIRECTORY)
                            .unwrap()
                            .join(&Self::encode_name(&album.title))
                            .map_err(|error| Self::provider_error(error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|path| RemoteEntry {
                        metadata: Self::root_metadata(path),
                    })
                    .collect(),
                next_cursor: page.next_cursor,
            });
        }
        if root == ALBUMS_DIRECTORY && components.len() == 1 {
            let album = self
                .find_album(
                    &Self::encode_name(components[0])
                        .replace("%2F", "/")
                        .replace("%5C", "\\")
                        .replace("%25", "%"),
                )
                .await?;
            let page = self.list_media(Some(&album.id), cursor).await?;
            return Ok(Page {
                entries: page
                    .entries
                    .into_iter()
                    .map(|item| {
                        let item_path = Self::item_path(prefix, &item)?;
                        Ok(RemoteEntry {
                            metadata: Self::media_metadata(item, item_path),
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?,
                next_cursor: page.next_cursor,
            });
        }
        Err(StorageError::NotFound {
            path: prefix.clone(),
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let (root, components) = Self::split_path(path);
        if root.is_empty()
            || (components.is_empty() && matches!(root, ALL_PHOTOS_DIRECTORY | ALBUMS_DIRECTORY))
        {
            return Ok(Self::root_metadata(path.clone()));
        }
        if root == ALL_PHOTOS_DIRECTORY && components.len() == 1 {
            let item = self.find_media(None, components[0], path).await?;
            return Ok(Self::media_metadata(item, path.clone()));
        }
        if root == ALBUMS_DIRECTORY && components.len() == 1 {
            self.find_album(
                &Self::encode_name(components[0])
                    .replace("%2F", "/")
                    .replace("%5C", "\\")
                    .replace("%25", "%"),
            )
            .await?;
            return Ok(Self::root_metadata(path.clone()));
        }
        if root == ALBUMS_DIRECTORY && components.len() == 2 {
            let album = self
                .find_album(
                    &Self::encode_name(components[0])
                        .replace("%2F", "/")
                        .replace("%5C", "\\")
                        .replace("%25", "%"),
                )
                .await?;
            let item = self
                .find_media(Some(&album.id), components[1], path)
                .await?;
            return Ok(Self::media_metadata(item, path.clone()));
        }
        Err(StorageError::NotFound { path: path.clone() })
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let (root, components) = Self::split_path(&request.path);
        let item = if root == ALL_PHOTOS_DIRECTORY && components.len() == 1 {
            self.find_media(None, components[0], &request.path).await?
        } else if root == ALBUMS_DIRECTORY && components.len() == 2 {
            let album = self
                .find_album(
                    &Self::encode_name(components[0])
                        .replace("%2F", "/")
                        .replace("%5C", "\\")
                        .replace("%25", "%"),
                )
                .await?;
            self.find_media(Some(&album.id), components[1], &request.path)
                .await?
        } else {
            return Err(StorageError::Unsupported {
                provider: self.kind(),
                capability: "read_directory".to_owned(),
            });
        };
        let response = self
            .client
            .get(format!(
                "{}{}",
                item.base_url,
                Self::range_suffix(request.range)?
            ))
            .send()
            .await
            .map_err(Self::network_error)?;
        let response = self.send(response).await?;
        Ok(Box::pin(
            response
                .bytes_stream()
                .map(|chunk| chunk.map_err(Self::network_error)),
        ))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let (root, components) = Self::split_path(&request.path);
        if root == ALL_PHOTOS_DIRECTORY && components.len() == 1 {
            return self.upload(request, None).await;
        }
        if root == ALBUMS_DIRECTORY && components.len() == 2 {
            let album = self
                .find_album(
                    &Self::encode_name(components[0])
                        .replace("%2F", "/")
                        .replace("%5C", "\\")
                        .replace("%25", "%"),
                )
                .await?;
            return self.upload(request, Some(&album.id)).await;
        }
        Err(Self::provider_error(
            "Google Photos uploads must target All Photos or an app-created album",
        ))
    }

    async fn delete(&self, _path: &RemotePath) -> Result<(), StorageError> {
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "delete".to_owned(),
        })
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        let (root, components) = Self::split_path(path);
        if root != ALBUMS_DIRECTORY || components.len() != 1 {
            return Err(StorageError::Unsupported {
                provider: self.kind(),
                capability: "create_directory".to_owned(),
            });
        }
        let title = Self::encode_name(components[0])
            .replace("%2F", "/")
            .replace("%5C", "\\")
            .replace("%25", "%");
        let access_token = self.access_token().await?;
        self.send(
            self.authorized(Method::POST, self.api_url("albums"), &access_token)
                .json(&CreateAlbumRequest {
                    album: CreateAlbum { title: &title },
                })
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await?;
        Ok(())
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let (from_root, from_components) = Self::split_path(from);
        let (to_root, to_components) = Self::split_path(to);
        if from_root != ALBUMS_DIRECTORY
            || to_root != ALBUMS_DIRECTORY
            || from_components.len() != 1
            || to_components.len() != 1
        {
            return Err(StorageError::Unsupported {
                provider: self.kind(),
                capability: "rename".to_owned(),
            });
        }
        let album = self
            .find_album(
                &Self::encode_name(from_components[0])
                    .replace("%2F", "/")
                    .replace("%5C", "\\")
                    .replace("%25", "%"),
            )
            .await?;
        let title = Self::encode_name(to_components[0])
            .replace("%2F", "/")
            .replace("%5C", "\\")
            .replace("%25", "%");
        let access_token = self.access_token().await?;
        self.send(
            self.authorized(
                Method::PATCH,
                self.api_url(&format!("albums/{}", album.id)),
                &access_token,
            )
            .query(&[("updateMask", "title")])
            .json(&UpdateAlbumRequest { title: &title })
            .send()
            .await
            .map_err(Self::network_error)?,
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl StorageProvider for HybridGooglePhotosProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GooglePhotos
    }

    fn capabilities(&self) -> CapabilitySet {
        let mut capabilities = self.photos.capabilities();
        if let Some(legacy) = self.legacy.as_ref() {
            capabilities = CapabilitySet::with(
                capabilities
                    .iter()
                    .copied()
                    .chain(legacy.capabilities().iter().copied()),
            );
        }
        capabilities
    }

    fn capabilities_for_path(&self, path: &RemotePath) -> CapabilitySet {
        if Self::is_legacy(path) {
            return self
                .legacy
                .as_ref()
                .map(|legacy| {
                    legacy.capabilities_for_path(
                        &Self::legacy_path(path).unwrap_or_else(|_| RemotePath::root()),
                    )
                })
                .unwrap_or_default();
        }
        self.photos.capabilities_for_path(path)
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        self.photos.test_connection().await?;
        if let Some(legacy) = self.legacy.as_ref() {
            legacy.test_connection().await?;
        }
        Ok(())
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        if prefix == &RemotePath::root() {
            let mut page = self.photos.list(prefix, cursor).await?;
            if self.legacy.is_some() && cursor.is_none() {
                page.entries.push(RemoteEntry {
                    metadata: GooglePhotosProvider::root_metadata(
                        RemotePath::parse(LEGACY_DIRECTORY).unwrap(),
                    ),
                });
            }
            return Ok(page);
        }
        if Self::is_legacy(prefix) {
            let legacy = self.legacy_provider(prefix)?;
            let path = Self::legacy_path(prefix)?;
            let page = legacy.list(&path, cursor).await?;
            return Ok(Page {
                entries: page
                    .entries
                    .into_iter()
                    .map(|entry| {
                        Ok(RemoteEntry {
                            metadata: Self::prefixed_legacy_metadata(entry.metadata)?,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?,
                next_cursor: page.next_cursor,
            });
        }
        self.photos.list(prefix, cursor).await
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        if path.as_str() == LEGACY_DIRECTORY {
            self.legacy_provider(path)?;
            return Ok(GooglePhotosProvider::root_metadata(path.clone()));
        }
        if Self::is_legacy(path) {
            return Self::prefixed_legacy_metadata(
                self.legacy_provider(path)?
                    .stat(&Self::legacy_path(path)?)
                    .await?,
            );
        }
        self.photos.stat(path).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        if Self::is_legacy(&request.path) {
            return self
                .legacy_provider(&request.path)?
                .read(ReadRequest {
                    path: Self::legacy_path(&request.path)?,
                    range: request.range,
                })
                .await;
        }
        self.photos.read(request).await
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        if Self::is_legacy(&request.path) {
            let path = Self::legacy_path(&request.path)?;
            let metadata = self
                .legacy_provider(&request.path)?
                .write(WriteRequest {
                    path,
                    content: request.content,
                    size_bytes: request.size_bytes,
                    modified_at: request.modified_at,
                })
                .await?;
            return Self::prefixed_legacy_metadata(metadata);
        }
        self.photos.write(request).await
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        if Self::is_legacy(path) {
            return self
                .legacy_provider(path)?
                .delete(&Self::legacy_path(path)?)
                .await;
        }
        self.photos.delete(path).await
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        if Self::is_legacy(path) {
            return self
                .legacy_provider(path)?
                .create_directory(&Self::legacy_path(path)?)
                .await;
        }
        self.photos.create_directory(path).await
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        if Self::is_legacy(from) && Self::is_legacy(to) {
            return self
                .legacy_provider(from)?
                .rename(&Self::legacy_path(from)?, &Self::legacy_path(to)?)
                .await;
        }
        if Self::is_legacy(from) || Self::is_legacy(to) {
            return Err(StorageError::Unsupported {
                provider: self.kind(),
                capability: "cross_source_rename".to_owned(),
            });
        }
        self.photos.rename(from, to).await
    }

    async fn copy(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        if Self::is_legacy(from) && Self::is_legacy(to) {
            return self
                .legacy_provider(from)?
                .copy(&Self::legacy_path(from)?, &Self::legacy_path(to)?)
                .await;
        }
        Err(StorageError::Unsupported {
            provider: self.kind(),
            capability: "cross_source_copy".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GooglePhotosProvider;

    #[test]
    fn escapes_virtual_file_names_without_double_encoding() {
        assert_eq!(
            GooglePhotosProvider::encode_name("holiday/100%\\original.jpg"),
            "holiday%2F100%25%5Coriginal.jpg"
        );
    }
}
