use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use bifrost_common::{RemoteMetadata, RemotePath};
use bifrost_storage::StorageError;
use tokio::runtime::Runtime;
use winfsp_wrs::{
    filetime_from_utc, filetime_now, u16cstr, CleanupFlags, CreateFileInfo, CreateOptions, DirInfo,
    FileAccessRights, FileAttributes, FileInfo, FileSystem, FileSystemInterface,
    PSecurityDescriptor, Params, SecurityDescriptor, U16CStr, U16CString, VolumeInfo, VolumeParams,
    WriteMode, NTSTATUS, STATUS_ACCESS_DENIED, STATUS_DIRECTORY_NOT_EMPTY, STATUS_END_OF_FILE,
    STATUS_FILE_IS_A_DIRECTORY, STATUS_INVALID_PARAMETER, STATUS_IO_DEVICE_ERROR,
    STATUS_NOT_A_DIRECTORY, STATUS_NOT_SUPPORTED, STATUS_OBJECT_NAME_COLLISION,
    STATUS_OBJECT_NAME_NOT_FOUND,
};

use crate::{
    remote_path_from_windows, MountConfig, OpenFile, RemoteFilesystem, WinFspError,
    WinFspFilesystemError,
};

const UNKNOWN_VOLUME_SIZE: u64 = 1 << 50;
static NEXT_MOUNT_ID: AtomicU64 = AtomicU64::new(1);

pub struct MountHandle {
    filesystem: Option<FileSystem>,
}

impl MountHandle {
    pub fn unmount(mut self) {
        if let Some(filesystem) = self.filesystem.take() {
            stop_filesystem(filesystem);
        }
    }
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        if let Some(filesystem) = self.filesystem.take() {
            stop_filesystem(filesystem);
        }
    }
}

fn stop_filesystem(filesystem: FileSystem) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filesystem.stop())).is_err() {
        tracing::error!("WinFsp filesystem teardown panicked");
    }
}

struct BifrostFileSystem {
    engine: RemoteFilesystem,
    runtime: Runtime,
    volume_label: U16CString,
    volume_total_bytes: Arc<AtomicU64>,
    volume_available_bytes: Arc<AtomicU64>,
    security: SecurityDescriptor,
}

impl BifrostFileSystem {
    fn block_on<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, WinFspFilesystemError>>,
    ) -> Result<T, NTSTATUS> {
        self.runtime.block_on(future).map_err(status_from_error)
    }

    fn path(file_name: &U16CStr) -> Result<RemotePath, NTSTATUS> {
        remote_path_from_windows(&file_name.to_string_lossy()).map_err(status_from_error)
    }

    fn existing_path(&self, file_name: &U16CStr) -> Result<RemotePath, NTSTATUS> {
        let path = Self::path(file_name)?;
        self.block_on(self.engine.resolve_case_insensitive(&path, false))
    }

    fn destination_path(&self, file_name: &U16CStr) -> Result<RemotePath, NTSTATUS> {
        let path = Self::path(file_name)?;
        self.block_on(self.engine.resolve_case_insensitive(&path, true))
    }

    fn metadata_for_handle(&self, handle: &OpenFile) -> Result<RemoteMetadata, NTSTATUS> {
        let path = self.runtime.block_on(handle.path());
        if handle.is_writable() {
            let size = self.block_on(self.engine.file_size(handle))?;
            return Ok(RemoteMetadata {
                path,
                is_directory: handle.is_directory(),
                size_bytes: Some(size),
                etag: None,
                modified_at: None,
            });
        }
        self.block_on(self.engine.stat(&path))
    }

    fn file_info(metadata: &RemoteMetadata) -> FileInfo {
        let mut info = FileInfo::default();
        let size = metadata.size_bytes.unwrap_or(0);
        let mut attributes = if metadata.is_directory {
            FileAttributes::DIRECTORY
        } else {
            FileAttributes::ARCHIVE
        };
        if metadata
            .path
            .as_str()
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with('.'))
        {
            attributes |= FileAttributes::HIDDEN;
        }
        let timestamp = metadata
            .modified_at
            .map(filetime_from_utc)
            .unwrap_or_else(filetime_now);
        info.set_file_attributes(attributes)
            .set_file_size(size)
            .set_allocation_size(size.div_ceil(4096) * 4096)
            .set_time(timestamp);
        info
    }

    fn writable(access: FileAccessRights) -> bool {
        access.is(FileAccessRights::FILE_WRITE_DATA)
            || access.is(FileAccessRights::FILE_APPEND_DATA)
            || access.is(FileAccessRights::FILE_WRITE_ATTRIBUTES)
    }
}

fn network_prefix(volume_label: &str) -> String {
    let share = volume_label
        .chars()
        .map(|character| match character {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();
    let share = share.trim().trim_matches('.');
    let share = if share.is_empty() {
        "Bifrost Drive"
    } else {
        share
    };
    let mount_id = NEXT_MOUNT_ID.fetch_add(1, Ordering::Relaxed);
    format!(r"\bifrost-{}-{mount_id}\{share}", std::process::id())
}

impl FileSystemInterface for BifrostFileSystem {
    type FileContext = Arc<OpenFile>;

    const GET_VOLUME_INFO_DEFINED: bool = true;
    fn get_volume_info(&self) -> Result<VolumeInfo, NTSTATUS> {
        VolumeInfo::new(
            self.volume_total_bytes.load(Ordering::Relaxed),
            self.volume_available_bytes.load(Ordering::Relaxed),
            self.volume_label.as_ustr(),
        )
        .map_err(|_| STATUS_INVALID_PARAMETER)
    }

    const GET_SECURITY_BY_NAME_DEFINED: bool = true;
    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _find_reparse_point: impl Fn() -> Option<FileAttributes>,
    ) -> Result<(FileAttributes, PSecurityDescriptor, bool), NTSTATUS> {
        let path = self.existing_path(file_name)?;
        let metadata = self.block_on(self.engine.stat(&path))?;
        let mut attributes = if metadata.is_directory {
            FileAttributes::DIRECTORY
        } else {
            FileAttributes::ARCHIVE
        };
        if path
            .as_str()
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with('.'))
        {
            attributes |= FileAttributes::HIDDEN;
        }
        Ok((attributes, self.security.as_ptr(), false))
    }

    const CREATE_DEFINED: bool = true;
    fn create(
        &self,
        file_name: &U16CStr,
        create_file_info: CreateFileInfo,
        _security_descriptor: SecurityDescriptor,
    ) -> Result<(Self::FileContext, FileInfo), NTSTATUS> {
        let path = self.destination_path(file_name)?;
        let handle = if create_file_info
            .create_options
            .is(CreateOptions::FILE_DIRECTORY_FILE)
        {
            self.block_on(self.engine.create_directory(path))?
        } else {
            self.block_on(self.engine.create_file(path))?
        };
        let metadata = self.metadata_for_handle(&handle)?;
        Ok((handle, Self::file_info(&metadata)))
    }

    const OPEN_DEFINED: bool = true;
    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: CreateOptions,
        granted_access: FileAccessRights,
    ) -> Result<(Self::FileContext, FileInfo), NTSTATUS> {
        let path = self.existing_path(file_name)?;
        let handle = self.block_on(self.engine.open(path, Self::writable(granted_access)))?;
        let metadata = self.metadata_for_handle(&handle)?;
        Ok((handle, Self::file_info(&metadata)))
    }

    const OVERWRITE_DEFINED: bool = true;
    fn overwrite(
        &self,
        file_context: Self::FileContext,
        _file_attributes: FileAttributes,
        _replace_file_attributes: bool,
        _allocation_size: u64,
    ) -> Result<FileInfo, NTSTATUS> {
        self.block_on(self.engine.set_file_size(&file_context, 0))?;
        Ok(Self::file_info(&self.metadata_for_handle(&file_context)?))
    }

    const CLEANUP_DEFINED: bool = true;
    fn cleanup(
        &self,
        file_context: Self::FileContext,
        _file_name: Option<&U16CStr>,
        flags: CleanupFlags,
    ) {
        if flags.is(CleanupFlags::DELETE) && file_context.delete_pending() {
            let _ = self.runtime.block_on(self.engine.delete(&file_context));
        } else if file_context.is_writable() {
            let _ = self.runtime.block_on(self.engine.flush(&file_context));
        }
    }

    const CLOSE_DEFINED: bool = true;
    fn close(&self, _file_context: Self::FileContext) {}

    const READ_DEFINED: bool = true;
    fn read(
        &self,
        file_context: Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> Result<usize, NTSTATUS> {
        let data = self.block_on(self.engine.read(&file_context, offset, buffer.len()))?;
        if data.is_empty() {
            return Err(STATUS_END_OF_FILE);
        }
        buffer[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    const WRITE_DEFINED: bool = true;
    fn write(
        &self,
        file_context: Self::FileContext,
        buffer: &[u8],
        mode: WriteMode,
    ) -> Result<(usize, FileInfo), NTSTATUS> {
        let written = match mode {
            WriteMode::Normal { offset } => {
                self.block_on(self.engine.write(&file_context, offset, buffer))?
            }
            WriteMode::WriteToEOF => self.block_on(self.engine.append(&file_context, buffer))?,
            WriteMode::ConstrainedIO { offset } => {
                let size = self.block_on(self.engine.file_size(&file_context))?;
                if offset >= size {
                    0
                } else {
                    let length = buffer.len().min((size - offset) as usize);
                    self.block_on(self.engine.write(&file_context, offset, &buffer[..length]))?
                }
            }
        };
        Ok((
            written,
            Self::file_info(&self.metadata_for_handle(&file_context)?),
        ))
    }

    const FLUSH_DEFINED: bool = true;
    fn flush(&self, file_context: Self::FileContext) -> Result<FileInfo, NTSTATUS> {
        let metadata = self.block_on(self.engine.flush(&file_context))?;
        Ok(Self::file_info(&metadata))
    }

    const GET_FILE_INFO_DEFINED: bool = true;
    fn get_file_info(&self, file_context: Self::FileContext) -> Result<FileInfo, NTSTATUS> {
        Ok(Self::file_info(&self.metadata_for_handle(&file_context)?))
    }

    const SET_FILE_SIZE_DEFINED: bool = true;
    fn set_file_size(
        &self,
        file_context: Self::FileContext,
        new_size: u64,
        set_allocation_size: bool,
    ) -> Result<FileInfo, NTSTATUS> {
        let current_size = self.block_on(self.engine.file_size(&file_context))?;
        if !set_allocation_size || new_size < current_size {
            self.block_on(self.engine.set_file_size(&file_context, new_size))?;
        }
        Ok(Self::file_info(&self.metadata_for_handle(&file_context)?))
    }

    const CAN_DELETE_DEFINED: bool = true;
    fn can_delete(
        &self,
        file_context: Self::FileContext,
        _file_name: &U16CStr,
    ) -> Result<(), NTSTATUS> {
        self.block_on(self.engine.can_delete(&file_context))
    }

    const RENAME_DEFINED: bool = true;
    fn rename(
        &self,
        file_context: Self::FileContext,
        _file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_exists: bool,
    ) -> Result<(), NTSTATUS> {
        let destination = self.destination_path(new_file_name)?;
        self.block_on(
            self.engine
                .rename(&file_context, destination, replace_if_exists),
        )
    }

    const GET_SECURITY_DEFINED: bool = true;
    fn get_security(
        &self,
        _file_context: Self::FileContext,
    ) -> Result<PSecurityDescriptor, NTSTATUS> {
        Ok(self.security.as_ptr())
    }

    const READ_DIRECTORY_DEFINED: bool = true;
    fn read_directory(
        &self,
        file_context: Self::FileContext,
        marker: Option<&U16CStr>,
        mut add_dir_info: impl FnMut(DirInfo) -> bool,
    ) -> Result<(), NTSTATUS> {
        if !file_context.is_directory() {
            return Err(STATUS_NOT_A_DIRECTORY);
        }
        let path = self.runtime.block_on(file_context.path());
        let mut entries = self.block_on(self.engine.list(&path))?;
        entries.sort_by_key(|entry| entry.metadata.path.as_str().to_lowercase());
        let marker = marker.map(U16CStr::to_string_lossy);
        for entry in entries {
            let Some(name) = entry.metadata.path.as_str().rsplit('/').next() else {
                continue;
            };
            if marker.as_deref().is_some_and(|value| name <= value) {
                continue;
            }
            if !add_dir_info(DirInfo::from_str(Self::file_info(&entry.metadata), name)) {
                break;
            }
        }
        Ok(())
    }

    const SET_DELETE_DEFINED: bool = true;
    fn set_delete(
        &self,
        file_context: Self::FileContext,
        _file_name: &U16CStr,
        delete_file: bool,
    ) -> Result<(), NTSTATUS> {
        if delete_file {
            self.block_on(self.engine.can_delete(&file_context))?;
        }
        file_context.set_delete_pending(delete_file);
        Ok(())
    }
}

pub fn mount(config: MountConfig) -> Result<MountHandle, WinFspError> {
    crate::initialize()?;
    let drive_letter = config.normalized_drive_letter()?;
    let mountpoint = U16CString::from_str(format!("{drive_letter}:"))
        .map_err(|error| WinFspError::Initialization(error.to_string()))?;
    let prefix = U16CString::from_str(network_prefix(&config.volume_label))
        .map_err(|error| WinFspError::Initialization(error.to_string()))?;
    let volume_label = U16CString::from_vec(
        config
            .volume_label
            .encode_utf16()
            .take(32)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| WinFspError::Initialization(error.to_string()))?;
    let security =
        SecurityDescriptor::from_wstr(u16cstr!("O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;WD)"))
            .map_err(WinFspError::Initialization)?;
    let runtime = Runtime::new().map_err(|error| WinFspError::Runtime(error.to_string()))?;
    let volume_total_bytes = Arc::new(AtomicU64::new(UNKNOWN_VOLUME_SIZE));
    let volume_available_bytes = Arc::new(AtomicU64::new(UNKNOWN_VOLUME_SIZE));
    let capacity_provider = Arc::clone(&config.provider);
    let capacity_total = Arc::clone(&volume_total_bytes);
    let capacity_available = Arc::clone(&volume_available_bytes);
    runtime.spawn(async move {
        if let Ok(Ok(Some(capacity))) =
            tokio::time::timeout(Duration::from_secs(10), capacity_provider.capacity()).await
        {
            capacity_total.store(capacity.total_bytes, Ordering::Relaxed);
            capacity_available.store(capacity.available_bytes, Ordering::Relaxed);
        }
    });

    let mut volume_params = VolumeParams::default();
    volume_params
        .set_sector_size(512)
        .set_sectors_per_allocation_unit(8)
        .set_volume_creation_time(filetime_now())
        .set_volume_serial_number(drive_letter as u32)
        .set_file_info_timeout(5000)
        .set_case_sensitive_search(false)
        .set_case_preserved_names(true)
        .set_unicode_on_disk(true)
        .set_persistent_acls(true)
        .set_post_cleanup_when_modified_only(true)
        .set_file_system_name(u16cstr!("Bifrost"))
        .map_err(|_| WinFspError::Initialization("filesystem name is too long".to_owned()))?
        .set_prefix(&prefix)
        .map_err(|_| WinFspError::Initialization("network prefix is too long".to_owned()))?;

    let context = BifrostFileSystem {
        engine: RemoteFilesystem::new(config.provider),
        runtime,
        volume_label,
        volume_total_bytes,
        volume_available_bytes,
        security,
    };
    let filesystem = FileSystem::start(
        Params {
            volume_params,
            ..Default::default()
        },
        Some(&mountpoint),
        context,
    )
    .map_err(WinFspError::Mount)?;
    Ok(MountHandle {
        filesystem: Some(filesystem),
    })
}

fn status_from_error(error: WinFspFilesystemError) -> NTSTATUS {
    match error {
        WinFspFilesystemError::Storage(StorageError::NotFound { .. }) => {
            STATUS_OBJECT_NAME_NOT_FOUND
        }
        WinFspFilesystemError::Storage(StorageError::PermissionDenied { .. })
        | WinFspFilesystemError::Storage(StorageError::AuthenticationFailed { .. }) => {
            STATUS_ACCESS_DENIED
        }
        WinFspFilesystemError::Storage(StorageError::Unsupported { .. }) => STATUS_NOT_SUPPORTED,
        WinFspFilesystemError::InvalidPath(_) => STATUS_INVALID_PARAMETER,
        WinFspFilesystemError::AlreadyExists(_) => STATUS_OBJECT_NAME_COLLISION,
        WinFspFilesystemError::IsDirectory(_) => STATUS_FILE_IS_A_DIRECTORY,
        WinFspFilesystemError::NotDirectory(_) => STATUS_NOT_A_DIRECTORY,
        WinFspFilesystemError::DirectoryNotEmpty(_) => STATUS_DIRECTORY_NOT_EMPTY,
        WinFspFilesystemError::NotWritable(_) => STATUS_ACCESS_DENIED,
        WinFspFilesystemError::Storage(_)
        | WinFspFilesystemError::Staging(_)
        | WinFspFilesystemError::Io(_) => STATUS_IO_DEVICE_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::{network_prefix, BifrostFileSystem};
    use bifrost_common::{RemoteMetadata, RemotePath};
    use winfsp_wrs::FileAttributes;

    #[test]
    fn marks_unix_dotfiles_hidden() {
        let metadata = RemoteMetadata {
            path: RemotePath::parse(".DAV").unwrap(),
            is_directory: true,
            size_bytes: Some(0),
            etag: None,
            modified_at: None,
        };

        let info = BifrostFileSystem::file_info(&metadata);
        assert!(info.file_attributes().is(FileAttributes::DIRECTORY));
        assert!(info.file_attributes().is(FileAttributes::HIDDEN));
    }

    #[test]
    fn network_prefixes_are_safe_named_and_unique() {
        let first = network_prefix("Yggdrasil:data");
        let second = network_prefix("Yggdrasil:data");

        assert!(first.ends_with(r"\Yggdrasil_data"));
        assert_ne!(first, second);
    }
}
