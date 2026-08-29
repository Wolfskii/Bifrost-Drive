use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageCapacity, StorageError, StorageProvider,
    WriteRequest,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt, TryStreamExt};
use reqwest::{header, Client, Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, ops::Range, sync::Arc};
use tokio::sync::Mutex;

const FOLDER_MIME_TYPE: &str = "application/vnd.google-apps.folder";
const FILE_FIELDS: &str =
    "nextPageToken,files(id,name,mimeType,size,modifiedTime,md5Checksum,version,webViewLink)";
const SINGLE_FILE_FIELDS: &str =
    "id,name,mimeType,size,modifiedTime,md5Checksum,version,webViewLink";
const MAX_WORKSPACE_EXPORT_CACHE_BYTES: usize = 64 * 1024 * 1024;
const GOOGLE_DOC_MIME_TYPE: &str = "application/vnd.google-apps.document";
const GOOGLE_SHEET_MIME_TYPE: &str = "application/vnd.google-apps.spreadsheet";
const GOOGLE_SLIDES_MIME_TYPE: &str = "application/vnd.google-apps.presentation";

#[derive(Clone, Copy)]
struct WorkspaceFormat {
    google_mime_type: &'static str,
    office_mime_type: &'static str,
    extension: &'static str,
}

#[derive(Clone, Copy)]
enum WorkspaceSelector {
    Binary,
    Any,
    Format(WorkspaceFormat),
}

const WORKSPACE_FORMATS: [WorkspaceFormat; 3] = [
    WorkspaceFormat {
        google_mime_type: GOOGLE_DOC_MIME_TYPE,
        office_mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        extension: ".docx",
    },
    WorkspaceFormat {
        google_mime_type: GOOGLE_SHEET_MIME_TYPE,
        office_mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        extension: ".xlsx",
    },
    WorkspaceFormat {
        google_mime_type: GOOGLE_SLIDES_MIME_TYPE,
        office_mime_type:
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        extension: ".pptx",
    },
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceOpenMode {
    #[default]
    NativeApps,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleDriveConfig {
    pub endpoint: Url,
    pub shared_drive_id: Option<String>,
    pub workspace_open_mode: WorkspaceOpenMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleDriveCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub expires_at: Option<i64>,
}

struct GoogleDriveSession {
    access_token: String,
    refresh_token: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    expires_at: Option<i64>,
}

pub struct GoogleDriveProvider {
    client: Client,
    endpoint: Url,
    shared_drive_id: Option<String>,
    workspace_open_mode: WorkspaceOpenMode,
    session: Arc<Mutex<GoogleDriveSession>>,
    workspace_exports: Arc<Mutex<HashMap<String, CachedWorkspaceExport>>>,
}

struct CachedWorkspaceExport {
    version: Option<String>,
    content: Option<Bytes>,
}

#[derive(Debug, Deserialize)]
struct FileListResponse {
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(default)]
    files: Vec<DriveFile>,
}

#[derive(Debug, Deserialize)]
struct DriveFile {
    id: String,
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
    size: Option<String>,
    #[serde(rename = "modifiedTime")]
    modified_time: Option<DateTime<Utc>>,
    #[serde(rename = "md5Checksum")]
    md5_checksum: Option<String>,
    version: Option<String>,
    #[serde(rename = "webViewLink")]
    web_view_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AboutResponse {
    #[serde(rename = "storageQuota")]
    storage_quota: Option<StorageQuota>,
}

#[derive(Debug, Deserialize)]
struct StorageQuota {
    limit: Option<String>,
    usage: Option<String>,
}

#[derive(Debug, Serialize)]
struct FileMutation<'a> {
    name: &'a str,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    mime_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<&'a str>>,
}

impl GoogleDriveProvider {
    pub fn connect(
        config: GoogleDriveConfig,
        access_token: impl Into<String>,
    ) -> Result<Self, StorageError> {
        Self::connect_with_credentials(
            config,
            GoogleDriveCredentials {
                access_token: access_token.into(),
                refresh_token: None,
                client_id: None,
                client_secret: None,
                expires_at: None,
            },
        )
    }

    pub fn connect_with_credentials(
        config: GoogleDriveConfig,
        credentials: GoogleDriveCredentials,
    ) -> Result<Self, StorageError> {
        if config.endpoint.scheme() != "https" {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "Google Drive endpoint must use HTTPS".to_owned(),
            });
        }
        if credentials.access_token.trim().is_empty() {
            return Err(StorageError::AuthenticationFailed {
                provider: ProviderKind::GoogleDrive,
            });
        }
        let client = Client::builder()
            .build()
            .map_err(|error| StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: error.to_string(),
            })?;
        Ok(Self {
            client,
            endpoint: config.endpoint,
            shared_drive_id: config.shared_drive_id,
            workspace_open_mode: config.workspace_open_mode,
            session: Arc::new(Mutex::new(GoogleDriveSession {
                access_token: credentials.access_token,
                refresh_token: credentials.refresh_token,
                client_id: credentials.client_id,
                client_secret: credentials.client_secret,
                expires_at: credentials.expires_at,
            })),
            workspace_exports: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn api_url(&self, path: &str) -> Result<Url, StorageError> {
        let mut url = self.endpoint.clone();
        let base = url.path().trim_end_matches('/');
        url.set_path(&format!("{base}/{path}"));
        Ok(url)
    }

    fn upload_url(&self, file_id: Option<&str>) -> Result<Url, StorageError> {
        let path = match file_id {
            Some(id) => format!("files/{id}"),
            None => "files".to_owned(),
        };
        let mut url = self.endpoint.clone();
        let base = self.endpoint.path().strip_suffix("/drive/v3").unwrap_or("");
        url.set_path(&format!("{base}/upload/drive/v3/{path}"));
        Ok(url)
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
                provider: ProviderKind::GoogleDrive,
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
        let error_message = response
            .text()
            .await
            .unwrap_or_else(|_| format!("server returned HTTP {status}"));
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                Err(StorageError::AuthenticationFailed {
                    provider: ProviderKind::GoogleDrive,
                })
            }
            StatusCode::NOT_FOUND => Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "remote item was not found".to_owned(),
            }),
            _ => Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: format!("server returned HTTP {status}: {error_message}"),
            }),
        }
    }

    async fn list_children(
        &self,
        parent_id: &str,
        name: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page<DriveFile>, StorageError> {
        let access_token = self.access_token().await?;
        let mut query = format!("'{parent_id}' in parents and trashed = false");
        if let Some(name) = name {
            query.push_str(" and name = '");
            query.push_str(&Self::escape_query_literal(name));
            query.push('\'');
        }
        let mut query_parameters = vec![
            ("q", query),
            ("pageSize", "1000".to_owned()),
            ("fields", FILE_FIELDS.to_owned()),
            ("includeItemsFromAllDrives", "true".to_owned()),
            ("supportsAllDrives", "true".to_owned()),
            ("spaces", "drive".to_owned()),
        ];
        if let Some(shared_drive_id) = self.shared_drive_id.as_ref() {
            query_parameters.push(("corpora", "drive".to_owned()));
            query_parameters.push(("driveId", shared_drive_id.clone()));
        } else {
            query_parameters.push(("corpora", "allDrives".to_owned()));
        }
        let mut request = self
            .authorized(Method::GET, self.api_url("files")?, &access_token)
            .query(&query_parameters);
        if let Some(cursor) = cursor {
            request = request.query(&[("pageToken", cursor)]);
        }
        let response = self
            .send(request.send().await.map_err(Self::network_error)?)
            .await?;
        let page = response
            .json::<FileListResponse>()
            .await
            .map_err(Self::network_error)?;
        Ok(Page {
            entries: page.files,
            next_cursor: page.next_page_token,
        })
    }

    async fn resolve_directory_id(&self, path: &RemotePath) -> Result<String, StorageError> {
        let mut parent_id = self
            .shared_drive_id
            .clone()
            .unwrap_or_else(|| "root".to_owned());
        for component in path
            .as_str()
            .split('/')
            .filter(|component| !component.is_empty())
        {
            let name = Self::decode_path_component(component)?;
            let page = self.list_children(&parent_id, Some(&name), None).await?;
            let folder = page
                .entries
                .into_iter()
                .find(|file| file.mime_type == FOLDER_MIME_TYPE)
                .ok_or_else(|| StorageError::NotFound { path: path.clone() })?;
            parent_id = folder.id;
        }
        Ok(parent_id)
    }

    async fn resolve_file(&self, path: &RemotePath) -> Result<DriveFile, StorageError> {
        if path == &RemotePath::root() {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "the Drive root has no file ID".to_owned(),
            });
        }
        let mut components = path.as_str().rsplitn(2, '/');
        let (name, workspace_selector) = Self::remote_file_name(
            components.next().unwrap_or_default(),
            self.workspace_open_mode,
        )?;
        let parent_path = components.next().unwrap_or_default();
        let parent = RemotePath::parse(parent_path).map_err(|error| StorageError::Provider {
            provider: ProviderKind::GoogleDrive,
            message: error.to_string(),
        })?;
        self.list_children(
            &self.resolve_directory_id(&parent).await?,
            Some(&name),
            None,
        )
        .await?
        .entries
        .into_iter()
        .find(|file| match workspace_selector {
            WorkspaceSelector::Binary => Self::workspace_format(&file.mime_type).is_none(),
            WorkspaceSelector::Any => Self::workspace_format(&file.mime_type).is_some(),
            WorkspaceSelector::Format(format) => file.mime_type == format.google_mime_type,
        })
        .ok_or_else(|| StorageError::NotFound { path: path.clone() })
    }

    fn escape_query_literal(value: &str) -> String {
        value.replace('\\', "\\\\").replace('\'', "\\'")
    }

    fn entry_path(
        prefix: &RemotePath,
        file: &DriveFile,
        open_mode: WorkspaceOpenMode,
    ) -> Result<RemotePath, StorageError> {
        let name = Self::virtual_file_name(file, open_mode);
        let path = if prefix.as_str().is_empty() {
            name
        } else {
            format!("{}/{}", prefix.as_str(), name)
        };
        RemotePath::parse(&path).map_err(|error| StorageError::Provider {
            provider: ProviderKind::GoogleDrive,
            message: error.to_string(),
        })
    }

    fn workspace_format(mime_type: &str) -> Option<WorkspaceFormat> {
        WORKSPACE_FORMATS
            .iter()
            .copied()
            .find(|format| format.google_mime_type == mime_type)
    }

    fn virtual_file_name(file: &DriveFile, open_mode: WorkspaceOpenMode) -> String {
        let mut name = Self::encode_path_component(&file.name);
        if let Some(format) = Self::workspace_format(&file.mime_type) {
            name.push_str(match open_mode {
                WorkspaceOpenMode::NativeApps => format.extension,
                WorkspaceOpenMode::Browser => ".url",
            });
            return name;
        }
        let lowercase = name.to_ascii_lowercase();
        let collision_extension = match open_mode {
            WorkspaceOpenMode::NativeApps => WORKSPACE_FORMATS.iter().find_map(|format| {
                lowercase
                    .ends_with(format.extension)
                    .then_some(format.extension)
            }),
            WorkspaceOpenMode::Browser => lowercase.ends_with(".url").then_some(".url"),
        };
        if let Some(extension) = collision_extension {
            let extension_start = name.len() - extension.len();
            name.replace_range(extension_start..extension_start + 1, "%2E");
        }
        name
    }

    fn remote_file_name(
        component: &str,
        open_mode: WorkspaceOpenMode,
    ) -> Result<(String, WorkspaceSelector), StorageError> {
        let lowercase = component.to_ascii_lowercase();
        match open_mode {
            WorkspaceOpenMode::NativeApps => {
                if let Some(format) = WORKSPACE_FORMATS
                    .iter()
                    .copied()
                    .find(|format| lowercase.ends_with(format.extension))
                {
                    let name = &component[..component.len() - format.extension.len()];
                    return Ok((
                        Self::decode_path_component(name)?,
                        WorkspaceSelector::Format(format),
                    ));
                }
            }
            WorkspaceOpenMode::Browser if lowercase.ends_with(".url") => {
                let name = &component[..component.len() - ".url".len()];
                return Ok((Self::decode_path_component(name)?, WorkspaceSelector::Any));
            }
            WorkspaceOpenMode::Browser => {}
        }
        Ok((
            Self::decode_path_component(component)?,
            WorkspaceSelector::Binary,
        ))
    }

    fn remote_destination_name(
        component: &str,
        workspace_format: Option<WorkspaceFormat>,
        open_mode: WorkspaceOpenMode,
    ) -> Result<String, StorageError> {
        let virtual_extension = workspace_format.map(|format| match open_mode {
            WorkspaceOpenMode::NativeApps => format.extension,
            WorkspaceOpenMode::Browser => ".url",
        });
        if let Some(extension) = virtual_extension
            .filter(|extension| component.to_ascii_lowercase().ends_with(extension))
        {
            return Self::decode_path_component(&component[..component.len() - extension.len()]);
        }
        Self::decode_path_component(component)
    }

    fn encode_path_component(value: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";

        let safe_end = value.trim_end_matches([' ', '.']).len();
        let reserved_name = Self::is_windows_reserved_name(value);
        let mut encoded = String::with_capacity(value.len());
        for (index, character) in value.char_indices() {
            let unsafe_character = character.is_ascii_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '%'
                )
                || index >= safe_end
                || (reserved_name && index == 0);
            if unsafe_character {
                let mut bytes = [0; 4];
                for byte in character.encode_utf8(&mut bytes).as_bytes() {
                    encoded.push('%');
                    encoded.push(HEX[(byte >> 4) as usize] as char);
                    encoded.push(HEX[(byte & 0x0f) as usize] as char);
                }
            } else {
                encoded.push(character);
            }
        }
        encoded
    }

    fn decode_path_component(value: &str) -> Result<String, StorageError> {
        let input = value.as_bytes();
        let mut decoded = Vec::with_capacity(input.len());
        let mut index = 0;
        while index < input.len() {
            if input[index] == b'%' {
                let Some(high) = input.get(index + 1).and_then(|byte| Self::hex_value(*byte))
                else {
                    return Err(Self::invalid_path_component(value));
                };
                let Some(low) = input.get(index + 2).and_then(|byte| Self::hex_value(*byte)) else {
                    return Err(Self::invalid_path_component(value));
                };
                decoded.push((high << 4) | low);
                index += 3;
            } else {
                decoded.push(input[index]);
                index += 1;
            }
        }
        String::from_utf8(decoded).map_err(|_| Self::invalid_path_component(value))
    }

    fn hex_value(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    fn is_windows_reserved_name(value: &str) -> bool {
        let stem = value
            .trim_end_matches([' ', '.'])
            .split('.')
            .next()
            .unwrap_or_default();
        matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
    }

    fn invalid_path_component(value: &str) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::GoogleDrive,
            message: format!("invalid encoded Google Drive path component: {value}"),
        }
    }

    async fn workspace_export(
        &self,
        file: &DriveFile,
        format: WorkspaceFormat,
    ) -> Result<Bytes, StorageError> {
        let mut cache = self.workspace_exports.lock().await;
        if let Some(content) = cache
            .get(&file.id)
            .filter(|cached| cached.version == file.version)
            .and_then(|cached| cached.content.clone())
        {
            return Ok(content);
        }
        let access_token = self.access_token().await?;
        let response = self
            .send(
                self.authorized(
                    Method::GET,
                    self.api_url(&format!("files/{}/export", file.id))?,
                    &access_token,
                )
                .query(&[("mimeType", format.office_mime_type)])
                .send()
                .await
                .map_err(Self::network_error)?,
            )
            .await?;
        let content = response.bytes().await.map_err(Self::network_error)?;
        Self::cache_workspace_export(
            &mut cache,
            file,
            content.clone(),
            MAX_WORKSPACE_EXPORT_CACHE_BYTES,
        );
        Ok(content)
    }

    fn cache_workspace_export(
        cache: &mut HashMap<String, CachedWorkspaceExport>,
        file: &DriveFile,
        content: Bytes,
        max_bytes: usize,
    ) {
        cache.remove(&file.id);
        let mut cached_bytes = cache
            .values()
            .filter_map(|cached| cached.content.as_ref())
            .map(Bytes::len)
            .sum::<usize>();
        while cached_bytes.saturating_add(content.len()) > max_bytes {
            let Some(key) = cache
                .iter()
                .find_map(|(key, cached)| cached.content.as_ref().map(|_| key.clone()))
            else {
                break;
            };
            if let Some(evicted) = cache.get_mut(&key).and_then(|cached| cached.content.take()) {
                cached_bytes = cached_bytes.saturating_sub(evicted.len());
            }
        }
        cache.insert(
            file.id.clone(),
            CachedWorkspaceExport {
                version: file.version.clone(),
                content: (content.len() <= max_bytes).then_some(content),
            },
        );
    }

    fn workspace_range(content: Bytes, range: Option<Range<u64>>) -> Result<Bytes, StorageError> {
        let Some(range) = range else {
            return Ok(content);
        };
        if range.start >= range.end {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "read range must have a positive length".to_owned(),
            });
        }
        let start = usize::try_from(range.start)
            .unwrap_or(usize::MAX)
            .min(content.len());
        let end = usize::try_from(range.end)
            .unwrap_or(usize::MAX)
            .min(content.len());
        Ok(content.slice(start..end.max(start)))
    }

    fn workspace_browser_shortcut(file: &DriveFile) -> Bytes {
        let url = file.web_view_link.clone().unwrap_or_else(|| {
            let mut url =
                Url::parse("https://drive.google.com/open").expect("Google Drive URL is valid");
            url.query_pairs_mut().append_pair("id", &file.id);
            url.into()
        });
        Bytes::from(format!("[InternetShortcut]\r\nURL={url}\r\n"))
    }

    fn metadata(file: DriveFile, path: RemotePath) -> RemoteMetadata {
        let is_workspace_file = Self::workspace_format(&file.mime_type).is_some();
        RemoteMetadata {
            path,
            is_directory: file.mime_type == FOLDER_MIME_TYPE,
            size_bytes: (!is_workspace_file)
                .then(|| file.size.and_then(|size| size.parse().ok()))
                .flatten(),
            etag: file.version.or(file.md5_checksum),
            modified_at: file.modified_time,
        }
    }

    fn range_header(range: Option<Range<u64>>) -> Result<Option<String>, StorageError> {
        let Some(range) = range else {
            return Ok(None);
        };
        if range.start >= range.end {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "read range must have a positive length".to_owned(),
            });
        }
        Ok(Some(format!("bytes={}-{}", range.start, range.end - 1)))
    }

    fn network_error(error: reqwest::Error) -> StorageError {
        StorageError::Network {
            provider: ProviderKind::GoogleDrive,
            message: error.to_string(),
        }
    }

    async fn ensure_parent(&self, path: &RemotePath) -> Result<(String, String), StorageError> {
        self.ensure_parent_for_format(path, None).await
    }

    async fn ensure_parent_for_format(
        &self,
        path: &RemotePath,
        workspace_format: Option<WorkspaceFormat>,
    ) -> Result<(String, String), StorageError> {
        let mut components = path.as_str().rsplitn(2, '/');
        let component = components.next().unwrap_or_default();
        let name =
            Self::remote_destination_name(component, workspace_format, self.workspace_open_mode)?;
        let parent_path =
            RemotePath::parse(components.next().unwrap_or_default()).map_err(|error| {
                StorageError::Provider {
                    provider: ProviderKind::GoogleDrive,
                    message: error.to_string(),
                }
            })?;
        Ok((self.resolve_directory_id(&parent_path).await?, name))
    }
}

#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    expires_in: i64,
}

#[async_trait]
impl StorageProvider for GoogleDriveProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GoogleDrive
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::Rename,
            Capability::ServerSideCopy,
            Capability::CreateDirectory,
            Capability::RangeRead,
        ])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        let access_token = self.access_token().await?;
        self.send(
            self.authorized(Method::GET, self.api_url("about")?, &access_token)
                .query(&[("fields", "user,storageQuota")])
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await?;
        self.list(&RemotePath::root(), None).await?;
        Ok(())
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let parent_id = self.resolve_directory_id(prefix).await?;
        let page = self.list_children(&parent_id, None, cursor).await?;
        let entries = page
            .entries
            .into_iter()
            .map(|file| {
                let path = Self::entry_path(prefix, &file, self.workspace_open_mode)?;
                Ok(RemoteEntry {
                    metadata: Self::metadata(file, path),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Page {
            entries,
            next_cursor: page.next_cursor,
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
        let file = self.resolve_file(path).await?;
        let export_size = if let Some(format) = Self::workspace_format(&file.mime_type) {
            Some(match self.workspace_open_mode {
                WorkspaceOpenMode::NativeApps => self.workspace_export(&file, format).await?.len(),
                WorkspaceOpenMode::Browser => Self::workspace_browser_shortcut(&file).len(),
            } as u64)
        } else {
            None
        };
        let mut metadata = Self::metadata(file, path.clone());
        if let Some(export_size) = export_size {
            metadata.size_bytes = Some(export_size);
        }
        Ok(metadata)
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let access_token = self.access_token().await?;
        let file = self.resolve_file(&request.path).await?;
        if file.mime_type == FOLDER_MIME_TYPE {
            return Err(StorageError::Unsupported {
                provider: ProviderKind::GoogleDrive,
                capability: "read_directory".to_owned(),
            });
        }
        if let Some(format) = Self::workspace_format(&file.mime_type) {
            let content = match self.workspace_open_mode {
                WorkspaceOpenMode::NativeApps => self.workspace_export(&file, format).await?,
                WorkspaceOpenMode::Browser => Self::workspace_browser_shortcut(&file),
            };
            let content = Self::workspace_range(content, request.range)?;
            return Ok(Box::pin(stream::once(async move { Ok(content) })));
        }
        let mut request_builder = self.authorized(
            Method::GET,
            self.api_url(&format!("files/{}", file.id))?,
            &access_token,
        );
        request_builder = request_builder.query(&[("alt", "media")]);
        if let Some(range) = Self::range_header(request.range)? {
            request_builder = request_builder.header(header::RANGE, range);
        }
        let response = self
            .send(request_builder.send().await.map_err(Self::network_error)?)
            .await?;
        Ok(Box::pin(
            response
                .bytes_stream()
                .map(|chunk| chunk.map_err(Self::network_error)),
        ))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let access_token = self.access_token().await?;
        let path = request.path;
        let uploaded_size = request.size_bytes;
        let existing = match self.resolve_file(&path).await {
            Ok(file) => Some(file),
            Err(StorageError::NotFound { .. }) => None,
            Err(error) => return Err(error),
        };
        if existing
            .as_ref()
            .is_some_and(|file| file.mime_type == FOLDER_MIME_TYPE)
        {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "cannot write content to a directory".to_owned(),
            });
        }
        let workspace_format = existing
            .as_ref()
            .and_then(|file| Self::workspace_format(&file.mime_type));
        if workspace_format.is_some() && self.workspace_open_mode == WorkspaceOpenMode::Browser {
            return Err(StorageError::Unsupported {
                provider: ProviderKind::GoogleDrive,
                capability: "write_workspace_browser_shortcut".to_owned(),
            });
        }
        if let (Some(file), Some(_)) = (existing.as_ref(), workspace_format) {
            let exports = self.workspace_exports.lock().await;
            let unchanged = exports
                .get(&file.id)
                .is_some_and(|cached| cached.version == file.version);
            if !unchanged {
                return Err(StorageError::Provider {
                    provider: ProviderKind::GoogleDrive,
                    message: format!(
                        "{} changed in Google Drive after it was opened; close and reopen the file before saving",
                        file.name
                    ),
                });
            }
        }
        let file_id = if let Some(file) = existing {
            file.id
        } else {
            let (parent_id, name) = self.ensure_parent(&path).await?;
            let response = self
                .send(
                    self.authorized(Method::POST, self.api_url("files")?, &access_token)
                        .json(&FileMutation {
                            name: &name,
                            mime_type: None,
                            parents: Some(vec![&parent_id]),
                        })
                        .send()
                        .await
                        .map_err(Self::network_error)?,
                )
                .await?;
            response
                .json::<DriveFile>()
                .await
                .map_err(Self::network_error)?
                .id
        };
        let response = self
            .send(
                self.authorized(
                    Method::PATCH,
                    self.upload_url(Some(&file_id))?,
                    &access_token,
                )
                .query(&[("uploadType", "media")])
                .query(&[("supportsAllDrives", "true")])
                .query(&[("fields", SINGLE_FILE_FIELDS)])
                .header(
                    header::CONTENT_TYPE,
                    workspace_format
                        .map_or("application/octet-stream", |format| format.office_mime_type),
                )
                .body(reqwest::Body::wrap_stream(
                    request.content.map_ok(|bytes| bytes),
                ))
                .send()
                .await
                .map_err(Self::network_error)?,
            )
            .await?;
        let file = response
            .json::<DriveFile>()
            .await
            .map_err(Self::network_error)?;
        if workspace_format.is_some() {
            self.workspace_exports.lock().await.insert(
                file_id,
                CachedWorkspaceExport {
                    version: file.version.clone(),
                    content: None,
                },
            );
        }
        let mut metadata = Self::metadata(file, path);
        if workspace_format.is_some() {
            metadata.size_bytes = uploaded_size;
        }
        Ok(metadata)
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let access_token = self.access_token().await?;
        let file = self.resolve_file(path).await?;
        let file_id = file.id;
        self.send(
            self.authorized(
                Method::DELETE,
                self.api_url(&format!("files/{file_id}"))?,
                &access_token,
            )
            .query(&[("supportsAllDrives", "true")])
            .send()
            .await
            .map_err(Self::network_error)?,
        )
        .await?;
        self.workspace_exports.lock().await.remove(&file_id);
        Ok(())
    }

    async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
        let access_token = self.access_token().await?;
        let response = self
            .send(
                self.authorized(Method::GET, self.api_url("about")?, &access_token)
                    .query(&[("fields", "storageQuota")])
                    .send()
                    .await
                    .map_err(Self::network_error)?,
            )
            .await?;
        let about = response
            .json::<AboutResponse>()
            .await
            .map_err(Self::network_error)?;
        let Some(quota) = about.storage_quota else {
            return Ok(None);
        };
        let Some(total) = quota.limit.and_then(|value| value.parse().ok()) else {
            return Ok(None);
        };
        let used = quota
            .usage
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Ok(Some(StorageCapacity {
            total_bytes: total,
            available_bytes: total.saturating_sub(used),
        }))
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        let access_token = self.access_token().await?;
        if path == &RemotePath::root() {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "the Drive root already exists".to_owned(),
            });
        }
        let (parent_id, name) = self.ensure_parent(path).await?;
        if self
            .list_children(&parent_id, Some(&name), None)
            .await?
            .entries
            .into_iter()
            .next()
            .is_some()
        {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "a remote item already exists at the destination".to_owned(),
            });
        }
        self.send(
            self.authorized(Method::POST, self.api_url("files")?, &access_token)
                .query(&[("supportsAllDrives", "true")])
                .json(&FileMutation {
                    name: &name,
                    mime_type: Some(FOLDER_MIME_TYPE),
                    parents: Some(vec![&parent_id]),
                })
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await
        .map(|_| ())
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let access_token = self.access_token().await?;
        let file = self.resolve_file(from).await?;
        let (parent_id, name) = self
            .ensure_parent_for_format(to, Self::workspace_format(&file.mime_type))
            .await?;
        if self
            .list_children(&parent_id, Some(&name), None)
            .await?
            .entries
            .into_iter()
            .next()
            .is_some()
        {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "a remote item already exists at the destination".to_owned(),
            });
        }
        let current_parent_path = RemotePath::parse(
            from.as_str()
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent),
        )
        .map_err(|error| StorageError::Provider {
            provider: ProviderKind::GoogleDrive,
            message: error.to_string(),
        })?;
        let current_parent = self.resolve_directory_id(&current_parent_path).await?;
        let mut request = self
            .authorized(
                Method::PATCH,
                self.api_url(&format!("files/{}", file.id))?,
                &access_token,
            )
            .query(&[("supportsAllDrives", "true")])
            .json(&FileMutation {
                name: &name,
                mime_type: None,
                parents: None,
            });
        if current_parent != parent_id {
            request =
                request.query(&[("addParents", parent_id), ("removeParents", current_parent)]);
        }
        self.send(request.send().await.map_err(Self::network_error)?)
            .await
            .map(|_| ())
    }

    async fn replace(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let destination = self.resolve_file(to).await?;
        let Some(workspace_format) = Self::workspace_format(&destination.mime_type) else {
            self.delete(to).await?;
            return self.rename(from, to).await;
        };
        if self.workspace_open_mode == WorkspaceOpenMode::Browser {
            return Err(StorageError::Unsupported {
                provider: ProviderKind::GoogleDrive,
                capability: "replace_workspace_browser_shortcut".to_owned(),
            });
        }
        let unchanged = self
            .workspace_exports
            .lock()
            .await
            .get(&destination.id)
            .is_some_and(|cached| cached.version == destination.version);
        if !unchanged {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: format!(
                    "{} changed in Google Drive after it was opened; close and reopen the file before saving",
                    destination.name
                ),
            });
        }

        let content = self
            .read(ReadRequest {
                path: from.clone(),
                range: None,
            })
            .await?;
        let access_token = self.access_token().await?;
        let response = self
            .send(
                self.authorized(
                    Method::PATCH,
                    self.upload_url(Some(&destination.id))?,
                    &access_token,
                )
                .query(&[("uploadType", "media")])
                .query(&[("supportsAllDrives", "true")])
                .query(&[("fields", SINGLE_FILE_FIELDS)])
                .header(header::CONTENT_TYPE, workspace_format.office_mime_type)
                .body(reqwest::Body::wrap_stream(content.map_ok(|bytes| bytes)))
                .send()
                .await
                .map_err(Self::network_error)?,
            )
            .await?;
        let updated = response
            .json::<DriveFile>()
            .await
            .map_err(Self::network_error)?;
        self.workspace_exports.lock().await.insert(
            destination.id,
            CachedWorkspaceExport {
                version: updated.version,
                content: None,
            },
        );
        self.delete(from).await
    }

    async fn copy(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let access_token = self.access_token().await?;
        let file = self.resolve_file(from).await?;
        let (parent_id, name) = self
            .ensure_parent_for_format(to, Self::workspace_format(&file.mime_type))
            .await?;
        if self
            .list_children(&parent_id, Some(&name), None)
            .await?
            .entries
            .into_iter()
            .next()
            .is_some()
        {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "a remote item already exists at the destination".to_owned(),
            });
        }
        self.send(
            self.authorized(
                Method::POST,
                self.api_url(&format!("files/{}/copy", file.id))?,
                &access_token,
            )
            .query(&[("supportsAllDrives", "true")])
            .json(&FileMutation {
                name: &name,
                mime_type: None,
                parents: Some(vec![&parent_id]),
            })
            .send()
            .await
            .map_err(Self::network_error)?,
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::{DriveFile, GoogleDriveProvider, WorkspaceOpenMode, GOOGLE_DOC_MIME_TYPE};
    use bifrost_common::RemotePath;
    use bytes::Bytes;
    use std::collections::HashMap;

    fn file(name: &str, mime_type: &str) -> DriveFile {
        DriveFile {
            id: "file-id".to_owned(),
            name: name.to_owned(),
            mime_type: mime_type.to_owned(),
            size: None,
            modified_time: None,
            md5_checksum: None,
            version: None,
            web_view_link: None,
        }
    }

    #[test]
    fn escapes_drive_query_literals() {
        assert_eq!(
            GoogleDriveProvider::escape_query_literal(r"owner\\s file's"),
            r"owner\\\\s file\'s"
        );
    }

    #[test]
    fn joins_nested_remote_paths() {
        let path = GoogleDriveProvider::entry_path(
            &RemotePath::parse("documents").unwrap(),
            &file("report.txt", "text/plain"),
            WorkspaceOpenMode::NativeApps,
        )
        .unwrap();
        assert_eq!(path.as_str(), "documents/report.txt");
    }

    #[test]
    fn encodes_google_names_that_are_not_safe_path_components() {
        let path = GoogleDriveProvider::entry_path(
            &RemotePath::root(),
            &file(r#"Report: Q3/100% \ draft?.txt"#, "text/plain"),
            WorkspaceOpenMode::NativeApps,
        )
        .unwrap();

        assert_eq!(path.as_str(), "Report%3A Q3%2F100%25 %5C draft%3F.txt");
        assert_eq!(
            GoogleDriveProvider::decode_path_component(path.as_str()).unwrap(),
            r#"Report: Q3/100% \ draft?.txt"#
        );
    }

    #[test]
    fn encodes_windows_reserved_names_and_trailing_characters() {
        for (name, encoded) in [
            ("CON.txt", "%43ON.txt"),
            ("quarterly report. ", "quarterly report%2E%20"),
            ("..", "%2E%2E"),
            ("räksmörgås.txt", "räksmörgås.txt"),
        ] {
            let path = GoogleDriveProvider::entry_path(
                &RemotePath::root(),
                &file(name, "application/octet-stream"),
                WorkspaceOpenMode::NativeApps,
            )
            .unwrap();
            assert_eq!(path.as_str(), encoded);
            assert_eq!(
                GoogleDriveProvider::decode_path_component(encoded).unwrap(),
                name
            );
        }
    }

    #[test]
    fn workspace_files_have_unambiguous_office_names() {
        let document = file("Project brief", GOOGLE_DOC_MIME_TYPE);
        let binary_document = file("Project brief.docx", "application/octet-stream");

        assert_eq!(
            GoogleDriveProvider::entry_path(
                &RemotePath::root(),
                &document,
                WorkspaceOpenMode::NativeApps,
            )
            .unwrap()
            .as_str(),
            "Project brief.docx"
        );
        assert_eq!(
            GoogleDriveProvider::entry_path(
                &RemotePath::root(),
                &binary_document,
                WorkspaceOpenMode::NativeApps,
            )
            .unwrap()
            .as_str(),
            "Project brief%2Edocx"
        );

        let (name, selector) = GoogleDriveProvider::remote_file_name(
            "Project brief.docx",
            WorkspaceOpenMode::NativeApps,
        )
        .unwrap();
        assert_eq!(name, "Project brief");
        assert!(
            matches!(selector, super::WorkspaceSelector::Format(format) if format.google_mime_type == GOOGLE_DOC_MIME_TYPE)
        );
        let (name, selector) = GoogleDriveProvider::remote_file_name(
            "Project brief%2Edocx",
            WorkspaceOpenMode::NativeApps,
        )
        .unwrap();
        assert_eq!(name, "Project brief.docx");
        assert!(matches!(selector, super::WorkspaceSelector::Binary));
    }

    #[test]
    fn workspace_listing_does_not_advertise_the_google_source_size() {
        let mut document = file("Project brief", GOOGLE_DOC_MIME_TYPE);
        document.size = Some("4412".to_owned());
        let metadata = GoogleDriveProvider::metadata(
            document,
            RemotePath::parse("Project brief.docx").unwrap(),
        );

        assert_eq!(metadata.size_bytes, None);
    }

    #[test]
    fn browser_mode_exposes_workspace_files_as_google_shortcuts() {
        let mut document = file("Project brief", GOOGLE_DOC_MIME_TYPE);
        document.web_view_link = Some("https://docs.google.com/document/d/file-id/edit".to_owned());
        let binary_shortcut = file("Project brief.url", "application/internet-shortcut");

        assert_eq!(
            GoogleDriveProvider::entry_path(
                &RemotePath::root(),
                &document,
                WorkspaceOpenMode::Browser,
            )
            .unwrap()
            .as_str(),
            "Project brief.url"
        );
        assert_eq!(
            GoogleDriveProvider::entry_path(
                &RemotePath::root(),
                &binary_shortcut,
                WorkspaceOpenMode::Browser,
            )
            .unwrap()
            .as_str(),
            "Project brief%2Eurl"
        );
        assert_eq!(
            GoogleDriveProvider::workspace_browser_shortcut(&document),
            Bytes::from_static(
                b"[InternetShortcut]\r\nURL=https://docs.google.com/document/d/file-id/edit\r\n"
            )
        );

        let (name, selector) =
            GoogleDriveProvider::remote_file_name("Project brief.url", WorkspaceOpenMode::Browser)
                .unwrap();
        assert_eq!(name, "Project brief");
        assert!(matches!(selector, super::WorkspaceSelector::Any));
    }

    #[test]
    fn workspace_destinations_drop_only_the_virtual_extension() {
        let format = GoogleDriveProvider::workspace_format(GOOGLE_DOC_MIME_TYPE).unwrap();

        assert_eq!(
            GoogleDriveProvider::remote_destination_name(
                "Renamed.docx",
                Some(format),
                WorkspaceOpenMode::NativeApps,
            )
            .unwrap(),
            "Renamed"
        );
        assert_eq!(
            GoogleDriveProvider::remote_destination_name(
                "Uploaded.docx",
                None,
                WorkspaceOpenMode::NativeApps,
            )
            .unwrap(),
            "Uploaded.docx"
        );
        assert_eq!(
            GoogleDriveProvider::remote_destination_name(
                "Renamed.url",
                Some(format),
                WorkspaceOpenMode::Browser,
            )
            .unwrap(),
            "Renamed"
        );
    }

    #[test]
    fn slices_cached_workspace_exports_for_filesystem_ranges() {
        let content = Bytes::from_static(b"exported document");

        assert_eq!(
            GoogleDriveProvider::workspace_range(content.clone(), Some(9..17)).unwrap(),
            Bytes::from_static(b"document")
        );
        assert!(GoogleDriveProvider::workspace_range(content, Some(30..40))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn workspace_cache_evicts_content_but_keeps_revision_baselines() {
        let mut cache = HashMap::new();
        let mut first = file("First", GOOGLE_DOC_MIME_TYPE);
        first.id = "first".to_owned();
        first.version = Some("1".to_owned());
        let mut second = file("Second", GOOGLE_DOC_MIME_TYPE);
        second.id = "second".to_owned();
        second.version = Some("2".to_owned());

        GoogleDriveProvider::cache_workspace_export(
            &mut cache,
            &first,
            Bytes::from_static(b"1234"),
            6,
        );
        GoogleDriveProvider::cache_workspace_export(
            &mut cache,
            &second,
            Bytes::from_static(b"5678"),
            6,
        );

        assert!(cache["first"].content.is_none());
        assert_eq!(cache["first"].version.as_deref(), Some("1"));
        assert_eq!(cache["second"].content.as_deref(), Some(b"5678".as_slice()));
    }
}
