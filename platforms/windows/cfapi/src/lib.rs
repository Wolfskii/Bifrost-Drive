use bifrost_common::RemoteMetadata;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CfapiError {
    #[error("Windows Cloud Files API is available only on Windows")]
    UnsupportedPlatform,
    #[error("CFAPI operation failed: {0}")]
    Platform(String),
    #[error("CFAPI callback channel is closed")]
    CallbackChannelClosed,
}

#[derive(Debug, Clone)]
pub struct PlaceholderMetadata {
    pub relative_name: String,
    pub remote: RemoteMetadata,
    pub identity: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfapiEvent {
    FetchData {
        file_identity: Vec<u8>,
        connection_key: i64,
        transfer_key: i64,
        request_key: i64,
        file_offset: i64,
        required_length: i64,
    },
    FetchPlaceholders {
        file_identity: Vec<u8>,
        connection_key: i64,
        transfer_key: i64,
        request_key: i64,
        pattern: Option<String>,
    },
    NotifyFileClose {
        file_identity: Vec<u8>,
    },
    NotifyDelete {
        file_identity: Vec<u8>,
    },
    NotifyRename {
        file_identity: Vec<u8>,
        target_path: String,
    },
}

#[derive(Debug, Clone)]
pub struct SyncRootConfig {
    pub path: PathBuf,
    pub provider_name: String,
    pub provider_version: String,
    pub provider_id: [u8; 16],
    pub sync_root_identity: Vec<u8>,
    pub root_file_identity: Vec<u8>,
}

#[cfg(not(target_os = "windows"))]
pub struct SyncRoot;

#[cfg(not(target_os = "windows"))]
impl SyncRoot {
    pub fn register(_config: SyncRootConfig) -> Result<Self, CfapiError> {
        Err(CfapiError::UnsupportedPlatform)
    }

    pub fn create_placeholders(&self, _entries: &[PlaceholderMetadata]) -> Result<u32, CfapiError> {
        Err(CfapiError::UnsupportedPlatform)
    }

    pub fn complete_fetch_data(
        _event: &CfapiEvent,
        _offset: i64,
        _data: &[u8],
    ) -> Result<(), CfapiError> {
        Err(CfapiError::UnsupportedPlatform)
    }

    pub fn fail_fetch_data(_event: &CfapiEvent, _status: i32) -> Result<(), CfapiError> {
        Err(CfapiError::UnsupportedPlatform)
    }

    pub fn complete_fetch_placeholders(
        _event: &CfapiEvent,
        _entries: &[PlaceholderMetadata],
    ) -> Result<u32, CfapiError> {
        Err(CfapiError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use std::{
        ffi::c_void,
        mem::size_of,
        os::windows::ffi::OsStrExt,
        sync::{mpsc, Arc},
    };
    use windows::{
        core::{GUID, PCWSTR},
        Win32::Foundation::NTSTATUS,
        Win32::Storage::{
            CloudFilters::{
                CfConnectSyncRoot, CfCreatePlaceholders, CfDisconnectSyncRoot, CfExecute,
                CfRegisterSyncRoot, CfUnregisterSyncRoot, CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS,
                CF_CALLBACK_REGISTRATION, CF_CALLBACK_TYPE_FETCH_DATA,
                CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS, CF_CALLBACK_TYPE_NONE,
                CF_CALLBACK_TYPE_NOTIFY_DELETE, CF_CALLBACK_TYPE_NOTIFY_FILE_CLOSE_COMPLETION,
                CF_CALLBACK_TYPE_NOTIFY_RENAME, CF_CONNECTION_KEY, CF_CONNECT_FLAGS,
                CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH, CF_CREATE_FLAGS, CF_FS_METADATA,
                CF_HARDLINK_POLICY_NONE, CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_MODIFIER,
                CF_HYDRATION_POLICY_MODIFIER_AUTO_DEHYDRATION_ALLOWED,
                CF_HYDRATION_POLICY_MODIFIER_STREAMING_ALLOWED, CF_HYDRATION_POLICY_PROGRESSIVE,
                CF_INSYNC_POLICY_TRACK_ALL, CF_OPERATION_INFO, CF_OPERATION_PARAMETERS,
                CF_OPERATION_PARAMETERS_0, CF_OPERATION_PARAMETERS_0_0,
                CF_OPERATION_PARAMETERS_0_4, CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_NONE, CF_OPERATION_TYPE_TRANSFER_DATA,
                CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS, CF_PLACEHOLDER_CREATE_FLAGS,
                CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC, CF_PLACEHOLDER_CREATE_INFO,
                CF_POPULATION_POLICY, CF_POPULATION_POLICY_MODIFIER, CF_POPULATION_POLICY_PARTIAL,
                CF_REGISTER_FLAGS, CF_REGISTER_FLAG_NONE, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION,
            },
            FileSystem::FILE_BASIC_INFO,
        },
    };

    pub struct SyncRoot {
        config: SyncRootConfig,
        connection_key: Option<windows::Win32::Storage::CloudFilters::CF_CONNECTION_KEY>,
        context: Box<CallbackContext>,
    }

    struct CallbackContext {
        sender: mpsc::Sender<CfapiEvent>,
        handler: Option<Arc<dyn Fn(CfapiEvent) + Send + Sync>>,
    }

    unsafe extern "system" fn fetch_data(
        info: *const CF_CALLBACK_INFO,
        _parameters: *const CF_CALLBACK_PARAMETERS,
    ) {
        if info.is_null() {
            return;
        }
        let info = unsafe { &*info };
        let Some(context) = (unsafe { (info.CallbackContext as *const CallbackContext).as_ref() })
        else {
            return;
        };
        let identity = unsafe {
            std::slice::from_raw_parts(
                info.FileIdentity.cast::<u8>(),
                info.FileIdentityLength as usize,
            )
        };
        let (file_offset, required_length) = if _parameters.is_null() {
            (0, 0)
        } else {
            let request = unsafe { (*_parameters).Anonymous.FetchData };
            (request.RequiredFileOffset, request.RequiredLength)
        };
        let event = CfapiEvent::FetchData {
            file_identity: identity.to_vec(),
            connection_key: info.ConnectionKey.0,
            transfer_key: info.TransferKey,
            request_key: info.RequestKey,
            file_offset,
            required_length,
        };
        if let Some(handler) = &context.handler {
            handler(event);
        } else {
            let _ = context.sender.send(event);
        }
    }

    unsafe extern "system" fn fetch_placeholders(
        info: *const CF_CALLBACK_INFO,
        parameters: *const CF_CALLBACK_PARAMETERS,
    ) {
        if info.is_null() {
            return;
        }
        let info = unsafe { &*info };
        let Some(context) = (unsafe { (info.CallbackContext as *const CallbackContext).as_ref() })
        else {
            return;
        };
        let identity = unsafe {
            std::slice::from_raw_parts(
                info.FileIdentity.cast::<u8>(),
                info.FileIdentityLength as usize,
            )
        };
        let pattern = if parameters.is_null() {
            None
        } else {
            let pattern = unsafe { (*parameters).Anonymous.FetchPlaceholders.Pattern };
            if pattern.is_null() {
                None
            } else {
                let length = (0..)
                    .take_while(|index| unsafe { *pattern.0.add(*index) != 0 })
                    .count();
                Some(String::from_utf16_lossy(unsafe {
                    std::slice::from_raw_parts(pattern.0, length)
                }))
            }
        };
        let event = CfapiEvent::FetchPlaceholders {
            file_identity: identity.to_vec(),
            connection_key: info.ConnectionKey.0,
            transfer_key: info.TransferKey,
            request_key: info.RequestKey,
            pattern,
        };
        if let Some(handler) = &context.handler {
            handler(event);
        } else {
            let _ = context.sender.send(event);
        }
    }

    unsafe extern "system" fn notify_file_close(
        info: *const CF_CALLBACK_INFO,
        _parameters: *const CF_CALLBACK_PARAMETERS,
    ) {
        notify(info, |identity| CfapiEvent::NotifyFileClose {
            file_identity: identity,
        });
    }

    unsafe extern "system" fn notify_delete(
        info: *const CF_CALLBACK_INFO,
        _parameters: *const CF_CALLBACK_PARAMETERS,
    ) {
        notify(info, |identity| CfapiEvent::NotifyDelete {
            file_identity: identity,
        });
    }

    unsafe extern "system" fn notify_rename(
        info: *const CF_CALLBACK_INFO,
        parameters: *const CF_CALLBACK_PARAMETERS,
    ) {
        if info.is_null() || parameters.is_null() {
            return;
        }
        let info = unsafe { &*info };
        let Some(context) = (unsafe { (info.CallbackContext as *const CallbackContext).as_ref() })
        else {
            return;
        };
        let identity = unsafe {
            std::slice::from_raw_parts(
                info.FileIdentity.cast::<u8>(),
                info.FileIdentityLength as usize,
            )
        };
        let target = unsafe { (*parameters).Anonymous.Rename.TargetPath };
        if target.is_null() {
            return;
        }
        let length = (0..)
            .take_while(|index| unsafe { *target.0.add(*index) != 0 })
            .count();
        dispatch(
            context,
            CfapiEvent::NotifyRename {
                file_identity: identity.to_vec(),
                target_path: String::from_utf16_lossy(unsafe {
                    std::slice::from_raw_parts(target.0, length)
                }),
            },
        );
    }

    unsafe fn notify(info: *const CF_CALLBACK_INFO, build: fn(Vec<u8>) -> CfapiEvent) {
        if info.is_null() {
            return;
        }
        let info = unsafe { &*info };
        let Some(context) = (unsafe { (info.CallbackContext as *const CallbackContext).as_ref() })
        else {
            return;
        };
        let identity = unsafe {
            std::slice::from_raw_parts(
                info.FileIdentity.cast::<u8>(),
                info.FileIdentityLength as usize,
            )
        };
        dispatch(context, build(identity.to_vec()));
    }

    fn dispatch(context: &CallbackContext, event: CfapiEvent) {
        if let Some(handler) = &context.handler {
            handler(event);
        } else {
            let _ = context.sender.send(event);
        }
    }

    static CALLBACKS: [CF_CALLBACK_REGISTRATION; 6] = [
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_DATA,
            Callback: Some(fetch_data),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
            Callback: Some(fetch_placeholders),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_CLOSE_COMPLETION,
            Callback: Some(notify_file_close),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_DELETE,
            Callback: Some(notify_delete),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_RENAME,
            Callback: Some(notify_rename),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NONE,
            Callback: None,
        },
    ];

    impl SyncRoot {
        pub fn register(config: SyncRootConfig) -> Result<Self, CfapiError> {
            let path = wide_path(&config.path);
            let provider_name = wide(&config.provider_name);
            let provider_version = wide(&config.provider_version);
            let provider_id = GUID::from_values(
                u32::from_le_bytes(config.provider_id[0..4].try_into().unwrap()),
                u16::from_le_bytes(config.provider_id[4..6].try_into().unwrap()),
                u16::from_le_bytes(config.provider_id[6..8].try_into().unwrap()),
                config.provider_id[8..16].try_into().unwrap(),
            );
            let registration = CF_SYNC_REGISTRATION {
                StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
                ProviderName: PCWSTR(provider_name.as_ptr()),
                ProviderVersion: PCWSTR(provider_version.as_ptr()),
                SyncRootIdentity: config.sync_root_identity.as_ptr().cast(),
                SyncRootIdentityLength: config.sync_root_identity.len() as u32,
                FileIdentity: config.root_file_identity.as_ptr().cast(),
                FileIdentityLength: config.root_file_identity.len() as u32,
                ProviderId: provider_id,
            };
            let policies = CF_SYNC_POLICIES {
                StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
                Hydration: CF_HYDRATION_POLICY {
                    Primary: CF_HYDRATION_POLICY_PROGRESSIVE,
                    Modifier: CF_HYDRATION_POLICY_MODIFIER(
                        CF_HYDRATION_POLICY_MODIFIER_STREAMING_ALLOWED.0
                            | CF_HYDRATION_POLICY_MODIFIER_AUTO_DEHYDRATION_ALLOWED.0,
                    ),
                },
                Population: CF_POPULATION_POLICY {
                    Primary: CF_POPULATION_POLICY_PARTIAL,
                    Modifier: CF_POPULATION_POLICY_MODIFIER(0),
                },
                InSync: CF_INSYNC_POLICY_TRACK_ALL,
                HardLink: CF_HARDLINK_POLICY_NONE,
                PlaceholderManagement: Default::default(),
            };
            unsafe {
                CfRegisterSyncRoot(
                    PCWSTR(path.as_ptr()),
                    &registration,
                    &policies,
                    CF_REGISTER_FLAGS(CF_REGISTER_FLAG_NONE.0),
                )
            }
            .map_err(|error| CfapiError::Platform(error.to_string()))?;
            Ok(Self {
                config,
                connection_key: None,
                context: Box::new(CallbackContext {
                    sender: mpsc::channel().0,
                    handler: None,
                }),
            })
        }

        pub fn connect(mut self) -> Result<(Self, mpsc::Receiver<CfapiEvent>), CfapiError> {
            let (sender, receiver) = mpsc::channel();
            self.context.sender = sender;
            self.context.handler = None;
            self.connect_native()?;
            Ok((self, receiver))
        }

        pub fn connect_with_handler(
            mut self,
            handler: Arc<dyn Fn(CfapiEvent) + Send + Sync>,
        ) -> Result<Self, CfapiError> {
            self.context.handler = Some(handler);
            self.connect_native()?;
            Ok(self)
        }

        fn connect_native(&mut self) -> Result<(), CfapiError> {
            let path = wide_path(&self.config.path);
            let connection = unsafe {
                CfConnectSyncRoot(
                    PCWSTR(path.as_ptr()),
                    CALLBACKS.as_ptr(),
                    Some((&*self.context as *const CallbackContext).cast::<c_void>()),
                    CF_CONNECT_FLAGS(CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH.0),
                )
            }
            .map_err(|error| CfapiError::Platform(error.to_string()))?;
            self.connection_key = Some(connection);
            Ok(())
        }

        pub fn create_placeholders(
            &self,
            entries: &[PlaceholderMetadata],
        ) -> Result<u32, CfapiError> {
            let base = wide_path(&self.config.path);
            let mut names = Vec::with_capacity(entries.len());
            let mut identities = Vec::with_capacity(entries.len());
            let mut infos = Vec::with_capacity(entries.len());
            for entry in entries {
                names.push(wide(&entry.relative_name));
                identities.push(entry.identity.clone());
            }
            for (index, entry) in entries.iter().enumerate() {
                let metadata = CF_FS_METADATA {
                    BasicInfo: FILE_BASIC_INFO::default(),
                    FileSize: entry.remote.size_bytes.unwrap_or(0) as i64,
                };
                infos.push(CF_PLACEHOLDER_CREATE_INFO {
                    RelativeFileName: PCWSTR(names[index].as_ptr()),
                    FsMetadata: metadata,
                    FileIdentity: identities[index].as_ptr().cast(),
                    FileIdentityLength: identities[index].len() as u32,
                    Flags: CF_PLACEHOLDER_CREATE_FLAGS(CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC.0),
                    ..Default::default()
                });
            }
            let mut processed = 0;
            unsafe {
                CfCreatePlaceholders(
                    PCWSTR(base.as_ptr()),
                    &mut infos,
                    CF_CREATE_FLAGS(0),
                    Some(&mut processed),
                )
            }
            .map_err(|error| CfapiError::Platform(error.to_string()))?;
            Ok(processed)
        }

        pub fn complete_fetch_data(
            event: &CfapiEvent,
            offset: i64,
            data: &[u8],
        ) -> Result<(), CfapiError> {
            Self::complete_fetch_data_with_status(event, offset, data, NTSTATUS(0))
        }

        pub fn fail_fetch_data(event: &CfapiEvent, status: i32) -> Result<(), CfapiError> {
            Self::complete_fetch_data_with_status(event, 0, &[], NTSTATUS(status))
        }

        fn complete_fetch_data_with_status(
            event: &CfapiEvent,
            offset: i64,
            data: &[u8],
            status: NTSTATUS,
        ) -> Result<(), CfapiError> {
            let CfapiEvent::FetchData {
                connection_key,
                transfer_key,
                request_key,
                ..
            } = event
            else {
                return Err(CfapiError::Platform(
                    "event is not a fetch-data callback".to_owned(),
                ));
            };
            let operation = CF_OPERATION_INFO {
                StructSize: size_of::<CF_OPERATION_INFO>() as u32,
                Type: CF_OPERATION_TYPE_TRANSFER_DATA,
                ConnectionKey: CF_CONNECTION_KEY(*connection_key),
                TransferKey: *transfer_key,
                RequestKey: *request_key,
                ..Default::default()
            };
            let transfer_data = CF_OPERATION_PARAMETERS_0_0 {
                Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CompletionStatus: status,
                Buffer: data.as_ptr().cast(),
                Offset: offset,
                Length: data.len() as i64,
            };
            let mut parameters = CF_OPERATION_PARAMETERS {
                ParamSize: size_of::<CF_OPERATION_PARAMETERS_0_0>() as u32,
                Anonymous: CF_OPERATION_PARAMETERS_0 {
                    TransferData: transfer_data,
                },
            };
            unsafe { CfExecute(&operation, &mut parameters) }
                .map_err(|error| CfapiError::Platform(error.to_string()))
        }

        pub fn complete_fetch_placeholders(
            event: &CfapiEvent,
            entries: &[PlaceholderMetadata],
        ) -> Result<u32, CfapiError> {
            let CfapiEvent::FetchPlaceholders {
                connection_key,
                transfer_key,
                request_key,
                ..
            } = event
            else {
                return Err(CfapiError::Platform(
                    "event is not a fetch-placeholders callback".to_owned(),
                ));
            };
            let mut names = Vec::with_capacity(entries.len());
            let mut identities = Vec::with_capacity(entries.len());
            let mut infos = Vec::with_capacity(entries.len());
            for entry in entries {
                names.push(wide(&entry.relative_name));
                identities.push(entry.identity.clone());
            }
            for (index, entry) in entries.iter().enumerate() {
                infos.push(CF_PLACEHOLDER_CREATE_INFO {
                    RelativeFileName: PCWSTR(names[index].as_ptr()),
                    FsMetadata: CF_FS_METADATA {
                        BasicInfo: FILE_BASIC_INFO::default(),
                        FileSize: entry.remote.size_bytes.unwrap_or(0) as i64,
                    },
                    FileIdentity: identities[index].as_ptr().cast(),
                    FileIdentityLength: identities[index].len() as u32,
                    Flags: CF_PLACEHOLDER_CREATE_FLAGS(CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC.0),
                    ..Default::default()
                });
            }
            let operation = CF_OPERATION_INFO {
                StructSize: size_of::<CF_OPERATION_INFO>() as u32,
                Type: CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS,
                ConnectionKey: CF_CONNECTION_KEY(*connection_key),
                TransferKey: *transfer_key,
                RequestKey: *request_key,
                ..Default::default()
            };
            let transfer_placeholders = CF_OPERATION_PARAMETERS_0_4 {
                Flags: CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_NONE,
                CompletionStatus: NTSTATUS(0),
                PlaceholderTotalCount: infos.len() as i64,
                PlaceholderArray: infos.as_mut_ptr(),
                PlaceholderCount: infos.len() as u32,
                EntriesProcessed: 0,
            };
            let mut parameters = CF_OPERATION_PARAMETERS {
                ParamSize: size_of::<CF_OPERATION_PARAMETERS_0_4>() as u32,
                Anonymous: CF_OPERATION_PARAMETERS_0 {
                    TransferPlaceholders: transfer_placeholders,
                },
            };
            unsafe { CfExecute(&operation, &mut parameters) }
                .map_err(|error| CfapiError::Platform(error.to_string()))?;
            Ok(infos.len() as u32)
        }
    }

    impl Drop for SyncRoot {
        fn drop(&mut self) {
            if let Some(connection) = self.connection_key.take() {
                let _ = unsafe { CfDisconnectSyncRoot(connection) };
            }
            let path = wide_path(&self.config.path);
            let _ = unsafe { CfUnregisterSyncRoot(PCWSTR(path.as_ptr())) };
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn wide_path(value: &std::path::Path) -> Vec<u16> {
        value
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::SyncRoot;
