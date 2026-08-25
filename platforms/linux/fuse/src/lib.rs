use bifrost_storage::StorageProvider;
use std::{path::PathBuf, sync::Arc};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct FuseConfig {
    pub mountpoint: PathBuf,
    pub filesystem_name: String,
}

#[derive(Debug, Error)]
pub enum FuseError {
    #[error("Linux FUSE is available only on Linux")]
    UnsupportedPlatform,
    #[error("FUSE mount failed: {0}")]
    Mount(#[from] std::io::Error),
    #[error("FUSE runtime initialization failed: {0}")]
    Runtime(String),
}

#[cfg(not(target_os = "linux"))]
pub struct MountHandle;

#[cfg(not(target_os = "linux"))]
pub fn mount_read_only(
    _provider: Arc<dyn StorageProvider>,
    _config: FuseConfig,
) -> Result<MountHandle, FuseError> {
    Err(FuseError::UnsupportedPlatform)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use bifrost_common::RemotePath;
    use bifrost_storage::{ReadRequest, StorageError};
    use fuser::{
        FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory,
        ReplyEntry, ReplyOpen, Request,
    };
    use std::{
        collections::HashMap,
        ffi::OsStr,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tokio::runtime::Runtime;

    const ROOT_INODE: u64 = 1;
    const TTL: Duration = Duration::from_secs(1);

    pub struct MountHandle {
        session: fuser::BackgroundSession,
    }

    struct RemoteFilesystem {
        provider: Arc<dyn StorageProvider>,
        runtime: Runtime,
        paths: HashMap<u64, RemotePath>,
        next_inode: u64,
    }

    impl RemoteFilesystem {
        fn new(provider: Arc<dyn StorageProvider>) -> Result<Self, FuseError> {
            let runtime = Runtime::new().map_err(|error| FuseError::Runtime(error.to_string()))?;
            let mut paths = HashMap::new();
            paths.insert(ROOT_INODE, RemotePath::root());
            Ok(Self {
                provider,
                runtime,
                paths,
                next_inode: ROOT_INODE + 1,
            })
        }

        fn inode_for(&mut self, path: RemotePath) -> u64 {
            if let Some((inode, _)) = self.paths.iter().find(|(_, known)| **known == path) {
                return *inode;
            }
            let inode = self.next_inode;
            self.next_inode += 1;
            self.paths.insert(inode, path);
            inode
        }

        fn path_for(&self, inode: u64) -> Option<RemotePath> {
            self.paths.get(&inode).cloned()
        }

        fn attributes(inode: u64, metadata: &bifrost_common::RemoteMetadata) -> FileAttr {
            let modified = metadata
                .modified_at
                .map(SystemTime::from)
                .unwrap_or(UNIX_EPOCH);
            FileAttr {
                ino: inode,
                size: metadata.size_bytes.unwrap_or(0),
                blocks: metadata.size_bytes.unwrap_or(0).div_ceil(512),
                atime: modified,
                mtime: modified,
                ctime: modified,
                crtime: modified,
                kind: if metadata.is_directory {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                perm: if metadata.is_directory { 0o555 } else { 0o444 },
                nlink: 1,
                uid: 0,
                gid: 0,
                rdev: 0,
                blksize: 4096,
                flags: 0,
            }
        }

        fn error_code(error: &StorageError) -> i32 {
            match error {
                StorageError::NotFound { .. } => libc::ENOENT,
                StorageError::PermissionDenied { .. } => libc::EACCES,
                _ => libc::EIO,
            }
        }
    }

    impl Filesystem for RemoteFilesystem {
        fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
            let Some(parent_path) = self.path_for(parent) else {
                reply.error(libc::ENOENT);
                return;
            };
            let Some(name) = name.to_str() else {
                reply.error(libc::EINVAL);
                return;
            };
            let Ok(path) = parent_path.join(name) else {
                reply.error(libc::EINVAL);
                return;
            };
            match self.runtime.block_on(self.provider.stat(&path)) {
                Ok(metadata) => {
                    let inode = self.inode_for(path);
                    reply.entry(&TTL, &Self::attributes(inode, &metadata), 0);
                }
                Err(error) => reply.error(Self::error_code(&error)),
            }
        }

        fn getattr(&mut self, _req: &Request<'_>, inode: u64, _fh: Option<u64>, reply: ReplyAttr) {
            let Some(path) = self.path_for(inode) else {
                reply.error(libc::ENOENT);
                return;
            };
            match self.runtime.block_on(self.provider.stat(&path)) {
                Ok(metadata) => reply.attr(&TTL, &Self::attributes(inode, &metadata)),
                Err(error) => reply.error(Self::error_code(&error)),
            }
        }

        fn open(&mut self, _req: &Request<'_>, inode: u64, _flags: i32, reply: ReplyOpen) {
            if self.paths.contains_key(&inode) {
                reply.opened(inode, 0);
            } else {
                reply.error(libc::ENOENT);
            }
        }

        fn readdir(
            &mut self,
            _req: &Request<'_>,
            inode: u64,
            _fh: u64,
            offset: i64,
            mut reply: ReplyDirectory,
        ) {
            let Some(path) = self.path_for(inode) else {
                reply.error(libc::ENOENT);
                return;
            };
            let page = match self.runtime.block_on(self.provider.list(&path, None)) {
                Ok(page) => page,
                Err(error) => {
                    reply.error(Self::error_code(&error));
                    return;
                }
            };
            let mut entries = vec![(inode, FileType::Directory, ".".to_owned())];
            let parent_inode = if inode == ROOT_INODE {
                ROOT_INODE
            } else {
                path.as_str()
                    .rsplit_once('/')
                    .and_then(|(parent, _)| RemotePath::parse(parent).ok())
                    .and_then(|parent| {
                        self.paths
                            .iter()
                            .find(|(_, known)| **known == parent)
                            .map(|(id, _)| *id)
                    })
                    .unwrap_or(ROOT_INODE)
            };
            entries.push((parent_inode, FileType::Directory, "..".to_owned()));
            for entry in page.entries {
                let entry_inode = self.inode_for(entry.metadata.path.clone());
                let kind = if entry.metadata.is_directory {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                };
                let Some(name) = entry.metadata.path.as_str().rsplit('/').next() else {
                    continue;
                };
                entries.push((entry_inode, kind, name.to_owned()));
            }
            for (index, (entry_inode, kind, name)) in
                entries.into_iter().enumerate().skip(offset.max(0) as usize)
            {
                if reply.add(entry_inode, (index + 1) as i64, kind, name) {
                    break;
                }
            }
            reply.ok();
        }

        fn read(
            &mut self,
            _req: &Request<'_>,
            inode: u64,
            _fh: u64,
            offset: i64,
            size: u32,
            _flags: i32,
            _lock_owner: Option<u64>,
            reply: ReplyData,
        ) {
            let Some(path) = self.path_for(inode) else {
                reply.error(libc::ENOENT);
                return;
            };
            if offset < 0 {
                reply.error(libc::EINVAL);
                return;
            }
            let result = self.runtime.block_on(async {
                let mut stream = self
                    .provider
                    .read(ReadRequest {
                        path,
                        range: Some(offset as u64..offset as u64 + size as u64),
                    })
                    .await?;
                let mut bytes = Vec::new();
                while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                    bytes.extend_from_slice(&chunk?);
                }
                Ok::<_, StorageError>(bytes)
            });
            match result {
                Ok(bytes) => reply.data(&bytes),
                Err(error) => reply.error(Self::error_code(&error)),
            }
        }
    }

    pub fn mount_read_only(
        provider: Arc<dyn StorageProvider>,
        config: FuseConfig,
    ) -> Result<MountHandle, FuseError> {
        let filesystem = RemoteFilesystem::new(provider)?;
        let options = [
            MountOption::RO,
            MountOption::FSName(config.filesystem_name),
            MountOption::DefaultPermissions,
        ];
        let session = fuser::spawn_mount2(filesystem, config.mountpoint, &options)?;
        Ok(MountHandle { session })
    }
}

#[cfg(target_os = "linux")]
pub use linux::{mount_read_only, MountHandle};
