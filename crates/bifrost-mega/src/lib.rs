use async_std::io::Cursor;
use async_trait::async_trait;
use bifrost_common::{Capability, CapabilitySet, ProviderKind, RemoteMetadata, RemotePath};
use bifrost_storage::{
    ByteStream, Page, ReadRequest, RemoteEntry, StorageCapacity, StorageError, StorageProvider,
    WriteRequest,
};
use bytes::Bytes;
use chrono::Utc;
use futures_util::StreamExt;
use mega::{Client, ClientBuilder, Node, Nodes};
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MegaConfig {
    pub email: String,
    pub password: String,
    pub root_path: String,
}

pub struct MegaProvider {
    config: MegaConfig,
    session: Mutex<Option<(Client, Nodes)>>,
}

impl MegaProvider {
    pub fn connect(config: MegaConfig) -> Result<Self, StorageError> {
        if config.email.trim().is_empty() || config.password.is_empty() {
            return Err(Self::error("MEGA email and password are required"));
        }
        Self::normalize_root_path(&config.root_path)?;
        Ok(Self {
            config,
            session: Mutex::new(None),
        })
    }

    fn error(message: impl Into<String>) -> StorageError {
        StorageError::Provider {
            provider: ProviderKind::Mega,
            message: message.into(),
        }
    }

    fn normalize_root_path(value: &str) -> Result<Vec<String>, StorageError> {
        let normalized = value.trim().replace('\\', "/");
        let mut components = Vec::new();
        for component in normalized.split('/') {
            match component {
                "" | "." => {}
                ".." => return Err(Self::error("MEGA start path cannot contain '..'")),
                component => components.push(component.to_owned()),
            }
        }
        Ok(components)
    }

    async fn session(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<(Client, Nodes)>>, StorageError> {
        let mut session = self.session.lock().await;
        if session.is_none() {
            let http = reqwest::Client::builder()
                .build()
                .map_err(|error| Self::error(error.to_string()))?;
            let mut client = ClientBuilder::new()
                .https(true)
                .build(http)
                .map_err(|error| Self::error(error.to_string()))?;
            client
                .login(&self.config.email, &self.config.password, None)
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            let nodes = client
                .fetch_own_nodes()
                .await
                .map_err(|error| Self::error(error.to_string()))?;
            *session = Some((client, nodes));
        }
        Ok(session)
    }

    fn node_for_path<'a>(nodes: &'a Nodes, path: &RemotePath) -> Result<&'a Node, StorageError> {
        let mut components = Self::normalize_root_path(path.as_str())?;
        let root = nodes
            .cloud_drive()
            .ok_or_else(|| Self::error("MEGA cloud drive root is unavailable"))?;
        if components.is_empty() {
            return Ok(root);
        }
        let mut node = root;
        while let Some(component) = components.first().cloned() {
            components.remove(0);
            node = nodes
                .get_node_by_handle(
                    node.children()
                        .iter()
                        .find_map(|handle| {
                            let child = nodes.get_node_by_handle(handle)?;
                            (child.name() == component).then_some(handle.as_str())
                        })
                        .ok_or_else(|| Self::error(format!("MEGA path not found: {path}")))?,
                )
                .ok_or_else(|| Self::error("MEGA node is unavailable"))?;
        }
        Ok(node)
    }

    fn joined_path(&self, path: &RemotePath) -> Result<RemotePath, StorageError> {
        let root = Self::normalize_root_path(&self.config.root_path)?;
        let child = Self::normalize_root_path(path.as_str())?;
        RemotePath::parse(&root.into_iter().chain(child).collect::<Vec<_>>().join("/"))
            .map_err(|error| Self::error(error.to_string()))
    }

    fn metadata(path: RemotePath, node: &Node) -> RemoteMetadata {
        RemoteMetadata {
            path,
            is_directory: node.kind().is_folder() || node.kind().is_root(),
            size_bytes: node.kind().is_file().then_some(node.size()),
            etag: None,
            modified_at: node.modified_at().or_else(|| Some(Utc::now())),
        }
    }

    fn child_nodes<'a>(nodes: &'a Nodes, node: &'a Node) -> impl Iterator<Item = &'a Node> {
        node.children()
            .iter()
            .filter_map(|handle| nodes.get_node_by_handle(handle))
    }
}

#[async_trait]
impl StorageProvider for MegaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Mega
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::with([
            Capability::Read,
            Capability::Write,
            Capability::Delete,
            Capability::Rename,
            Capability::CreateDirectory,
        ])
    }

    async fn test_connection(&self) -> Result<(), StorageError> {
        let _ = self.session().await?;
        Ok(())
    }

    async fn list(
        &self,
        prefix: &RemotePath,
        _cursor: Option<&str>,
    ) -> Result<Page<RemoteEntry>, StorageError> {
        let path = self.joined_path(prefix)?;
        let session = self.session().await?;
        let (_, nodes) = session
            .as_ref()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let parent = Self::node_for_path(nodes, &path)?;
        let entries = Self::child_nodes(nodes, parent)
            .map(|node| {
                let child_path = path
                    .join(node.name())
                    .map_err(|error| Self::error(error.to_string()))?;
                Ok(RemoteEntry {
                    metadata: Self::metadata(child_path, node),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Page {
            entries,
            next_cursor: None,
        })
    }

    async fn stat(&self, path: &RemotePath) -> Result<RemoteMetadata, StorageError> {
        let path = self.joined_path(path)?;
        let session = self.session().await?;
        let (_, nodes) = session
            .as_ref()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        Ok(Self::metadata(
            path.clone(),
            Self::node_for_path(nodes, &path)?,
        ))
    }

    async fn read(&self, request: ReadRequest) -> Result<ByteStream, StorageError> {
        let path = self.joined_path(&request.path)?;
        let mut session = self.session().await?;
        let (client, nodes) = session
            .as_mut()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let node = Self::node_for_path(nodes, &path)?;
        let mut output = Vec::new();
        client
            .download_node(&node, Cursor::new(&mut output))
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        let bytes = request.range.map_or_else(
            || Bytes::from(output.clone()),
            |range| {
                let start = range.start.min(output.len() as u64) as usize;
                let end = range.end.min(output.len() as u64) as usize;
                Bytes::copy_from_slice(&output[start..end.max(start)])
            },
        );
        Ok(Box::pin(futures_util::stream::once(
            async move { Ok(bytes) },
        )))
    }

    async fn write(&self, request: WriteRequest) -> Result<RemoteMetadata, StorageError> {
        let path = self.joined_path(&request.path)?;
        let mut content = Vec::new();
        let mut stream = request.content;
        while let Some(chunk) = stream.next().await {
            content.extend_from_slice(&chunk.map_err(|error| Self::error(error.to_string()))?);
        }
        let parent_path = RemotePath::parse(
            path.as_str()
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent),
        )
        .map_err(|error| Self::error(error.to_string()))?;
        let name = path.as_str().rsplit('/').next().unwrap_or_default();
        let mut session = self.session().await?;
        let (client, nodes) = session
            .as_mut()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let parent = Self::node_for_path(nodes, &parent_path)?;
        client
            .upload_node(
                &parent,
                name,
                content.len() as u64,
                Cursor::new(content),
                mega::LastModified::Now,
            )
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        Ok(RemoteMetadata {
            path: request.path,
            is_directory: false,
            size_bytes: request.size_bytes,
            etag: None,
            modified_at: request.modified_at,
        })
    }

    async fn delete(&self, path: &RemotePath) -> Result<(), StorageError> {
        let path = self.joined_path(path)?;
        let mut session = self.session().await?;
        let (client, nodes) = session
            .as_mut()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let node = Self::node_for_path(nodes, &path)?;
        client
            .delete_node(&node)
            .await
            .map_err(|error| Self::error(error.to_string()))
    }

    async fn capacity(&self) -> Result<Option<StorageCapacity>, StorageError> {
        let session = self.session().await?;
        let (client, _) = session
            .as_ref()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let quota = client
            .get_storage_quotas()
            .await
            .map_err(|error| Self::error(error.to_string()))?;
        Ok(Some(StorageCapacity {
            total_bytes: quota.memory_total,
            available_bytes: quota.memory_total.saturating_sub(quota.memory_used),
        }))
    }

    async fn create_directory(&self, path: &RemotePath) -> Result<(), StorageError> {
        let path = self.joined_path(path)?;
        let parent_path = RemotePath::parse(
            path.as_str()
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent),
        )
        .map_err(|error| Self::error(error.to_string()))?;
        let name = path.as_str().rsplit('/').next().unwrap_or_default();
        let mut session = self.session().await?;
        let (client, nodes) = session
            .as_mut()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let parent = Self::node_for_path(nodes, &parent_path)?;
        client
            .create_folder(&parent, name)
            .await
            .map_err(|error| Self::error(error.to_string()))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), StorageError> {
        let from = self.joined_path(from)?;
        let to = self.joined_path(to)?;
        let name = to.as_str().rsplit('/').next().unwrap_or_default();
        let mut session = self.session().await?;
        let (client, nodes) = session
            .as_mut()
            .ok_or_else(|| Self::error("MEGA session unavailable"))?;
        let node = Self::node_for_path(nodes, &from)?;
        client
            .rename_node(&node, name)
            .await
            .map_err(|error| Self::error(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{MegaConfig, MegaProvider};

    #[test]
    fn requires_email_and_password() {
        assert!(MegaProvider::connect(MegaConfig {
            email: String::new(),
            password: "password".to_owned(),
            root_path: String::new(),
        })
        .is_err());
        assert!(MegaProvider::connect(MegaConfig {
            email: "user@example.com".to_owned(),
            password: String::new(),
            root_path: String::new(),
        })
        .is_err());
    }

    #[test]
    fn rejects_parent_traversal_in_start_path() {
        assert!(MegaProvider::connect(MegaConfig {
            email: "user@example.com".to_owned(),
            password: "password".to_owned(),
            root_path: "documents/../private".to_owned(),
        })
        .is_err());
    }
}
