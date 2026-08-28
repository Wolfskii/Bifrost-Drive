use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageCapacity, StorageError, StorageProvider,
    WriteRequest,
};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, TryStreamExt};
use reqwest::{header, Client, Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::ops::Range;

const FOLDER_MIME_TYPE: &str = "application/vnd.google-apps.folder";
const FILE_FIELDS: &str = "nextPageToken,files(id,name,mimeType,size,modifiedTime,md5Checksum)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleDriveConfig {
    pub endpoint: Url,
}

pub struct GoogleDriveProvider {
    client: Client,
    endpoint: Url,
    access_token: String,
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
        if config.endpoint.scheme() != "https" {
            return Err(StorageError::Provider {
                provider: ProviderKind::GoogleDrive,
                message: "Google Drive endpoint must use HTTPS".to_owned(),
            });
        }
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
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
            access_token,
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

    fn authorized(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.access_token)
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
        let mut query = format!("'{parent_id}' in parents and trashed = false");
        if let Some(name) = name {
            query.push_str(" and name = '");
            query.push_str(&Self::escape_query_literal(name));
            query.push('\'');
        }
        let mut request = self
            .authorized(Method::GET, self.api_url("files")?)
            .query(&[
                ("q", query),
                ("pageSize", "1000".to_owned()),
                ("fields", FILE_FIELDS.to_owned()),
            ]);
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
        let mut parent_id = "root".to_owned();
        for component in path
            .as_str()
            .split('/')
            .filter(|component| !component.is_empty())
        {
            let page = self
                .list_children(&parent_id, Some(component), None)
                .await?;
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
        let name = components.next().unwrap_or_default();
        let parent_path = components.next().unwrap_or_default();
        let parent = RemotePath::parse(parent_path).map_err(|error| StorageError::Provider {
            provider: ProviderKind::GoogleDrive,
            message: error.to_string(),
        })?;
        self.list_children(&self.resolve_directory_id(&parent).await?, Some(name), None)
            .await?
            .entries
            .into_iter()
            .next()
            .ok_or_else(|| StorageError::NotFound { path: path.clone() })
    }

    fn escape_query_literal(value: &str) -> String {
        value.replace('\\', "\\\\").replace('\'', "\\'")
    }

    fn entry_path(prefix: &RemotePath, name: &str) -> Result<RemotePath, StorageError> {
        let path = if prefix.as_str().is_empty() {
            name.to_owned()
        } else {
            format!("{}/{}", prefix.as_str(), name)
        };
        RemotePath::parse(&path).map_err(|error| StorageError::Provider {
            provider: ProviderKind::GoogleDrive,
            message: error.to_string(),
        })
    }

    fn metadata(file: DriveFile, path: RemotePath) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: file.mime_type == FOLDER_MIME_TYPE,
            size_bytes: file.size.and_then(|size| size.parse().ok()),
            etag: file.md5_checksum,
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
        let mut components = path.as_str().rsplitn(2, '/');
        let name = components.next().unwrap_or_default().to_owned();
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
        self.send(
            self.authorized(Method::GET, self.api_url("about")?)
                .query(&[("fields", "user,storageQuota")])
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
        let parent_id = self.resolve_directory_id(prefix).await?;
        let page = self.list_children(&parent_id, None, cursor).await?;
        let entries = page
            .entries
            .into_iter()
            .map(|file| {
                let path = Self::entry_path(prefix, &file.name)?;
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
        Ok(Self::metadata(file, path.clone()))
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let file = self.resolve_file(&request.path).await?;
        if file.mime_type == FOLDER_MIME_TYPE {
            return Err(StorageError::Unsupported {
                provider: ProviderKind::GoogleDrive,
                capability: "read_directory".to_owned(),
            });
        }
        let mut request_builder =
            self.authorized(Method::GET, self.api_url(&format!("files/{}", file.id))?);
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
        let path = request.path;
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
        let file_id = if let Some(file) = existing {
            file.id
        } else {
            let (parent_id, name) = self.ensure_parent(&path).await?;
            let response = self
                .send(
                    self.authorized(Method::POST, self.api_url("files")?)
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
                self.authorized(Method::PATCH, self.upload_url(Some(&file_id))?)
                    .query(&[("uploadType", "media")])
                    .header(header::CONTENT_TYPE, "application/octet-stream")
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
        Ok(Self::metadata(file, path))
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let file = self.resolve_file(path).await?;
        self.send(
            self.authorized(Method::DELETE, self.api_url(&format!("files/{}", file.id))?)
                .send()
                .await
                .map_err(Self::network_error)?,
        )
        .await
        .map(|_| ())
    }

    async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
        let response = self
            .send(
                self.authorized(Method::GET, self.api_url("about")?)
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
            self.authorized(Method::POST, self.api_url("files")?)
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
        let file = self.resolve_file(from).await?;
        let (parent_id, name) = self.ensure_parent(to).await?;
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
            .authorized(Method::PATCH, self.api_url(&format!("files/{}", file.id))?)
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

    async fn copy(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let file = self.resolve_file(from).await?;
        let (parent_id, name) = self.ensure_parent(to).await?;
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
            )
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
    use super::GoogleDriveProvider;
    use bifrost_common::RemotePath;

    #[test]
    fn escapes_drive_query_literals() {
        assert_eq!(
            GoogleDriveProvider::escape_query_literal(r"owner\\s file's"),
            r"owner\\\\s file\'s"
        );
    }

    #[test]
    fn joins_nested_remote_paths() {
        let path =
            GoogleDriveProvider::entry_path(&RemotePath::parse("documents").unwrap(), "report.txt")
                .unwrap();
        assert_eq!(path.as_str(), "documents/report.txt");
    }
}
