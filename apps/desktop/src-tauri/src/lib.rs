#[cfg(target_os = "windows")]
use base64::Engine;
use bifrost_api::{
    ActivitySummary, AppStatus, ConnectionIdRequest, ConnectionSummary, CreateConnectionRequest,
    CreateFtpConnectionRequest, CreateS3ConnectionRequest, CreateSftpConnectionRequest,
    CreateSmbConnectionRequest, CreateWebDavConnectionRequest, CredentialSummary,
    DriveIconPreviewRequest, DriveMountRegisterRequest, DriveMountRegisterResponse,
    DriveMountStartupRequest, FilePage, FileSummary, HydrateFileRequest, HydrateFileResponse,
    ListFilesRequest, StoreS3CredentialRequest, SyncReconcileRequest, SyncReconcileResponse,
    SyncRunRequest, SyncRunResponse, TestConnectionRequest,
};
use bifrost_cache::{CacheManager, CacheRecord};
use bifrost_common::{ConnectionState, ProviderKind};
use bifrost_core::Application;
use bifrost_crypto::{CredentialError, CredentialRef, CredentialStore, SecretString};
use bifrost_db::{ConflictRecord, ConnectionRecord, Database, SyncEntryRecord};
use bifrost_ftp::{FtpConfig, FtpProvider};
#[cfg(target_os = "linux")]
use bifrost_linux_credentials::LinuxCredentialStore as WindowsCredentialStore;
#[cfg(target_os = "macos")]
use bifrost_macos_credentials::MacosCredentialStore as WindowsCredentialStore;
use bifrost_s3::{S3Config, S3Provider};
use bifrost_sftp::{SftpConfig, SftpProvider};
use bifrost_smb::{SmbConfig, SmbProvider};
use bifrost_storage::StorageProvider;
use bifrost_sync::{resolve, ConflictResolution, ReconciliationInput, Revision, SyncDecision};
use bifrost_transfer::TransferService;
use bifrost_transfer::{TransferDirection, TransferSnapshot, TransferStatus, TransferStore};
use bifrost_webdav::{WebDavConfig, WebDavProvider};
#[cfg(target_os = "windows")]
use bifrost_windows_cfapi::{CfapiEvent, PlaceholderMetadata, SyncRoot, SyncRootConfig};
#[cfg(target_os = "windows")]
use bifrost_windows_credentials::WindowsCredentialStore;
#[cfg(target_os = "windows")]
use bifrost_windows_winfsp::{MountConfig, MountHandle};
#[cfg(target_os = "windows")]
use image::ImageEncoder;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::sync::MutexGuard;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager, State,
};

#[cfg(target_os = "windows")]
struct SyncRootRegistry(Mutex<HashMap<String, SyncRoot>>);

#[cfg(target_os = "windows")]
struct DriveMountRegistry(Mutex<HashMap<String, MountHandle>>);

#[cfg(not(target_os = "windows"))]
struct DriveMountRegistry;

#[cfg(not(target_os = "windows"))]
struct SyncRootRegistry;

#[cfg(target_os = "windows")]
impl SyncRootRegistry {
    fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    fn roots(&self) -> MutexGuard<'_, HashMap<String, SyncRoot>> {
        self.0.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("recovering poisoned sync root registry");
            self.0.clear_poison();
            poisoned.into_inner()
        })
    }

    fn insert(&self, key: String, root: SyncRoot) -> Option<SyncRoot> {
        self.roots().insert(key, root)
    }

    fn take(&self, key: &str) -> Option<SyncRoot> {
        self.roots().remove(key)
    }
}

#[cfg(target_os = "windows")]
impl DriveMountRegistry {
    fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    fn mounts(&self) -> MutexGuard<'_, HashMap<String, MountHandle>> {
        self.0.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("recovering poisoned drive mount registry");
            self.0.clear_poison();
            poisoned.into_inner()
        })
    }

    fn contains(&self, key: &str) -> bool {
        self.mounts().contains_key(key)
    }

    fn insert(&self, key: String, handle: MountHandle) -> Option<MountHandle> {
        self.mounts().insert(key, handle)
    }

    fn take(&self, key: &str) -> Option<MountHandle> {
        self.mounts().remove(key)
    }
}

#[cfg(not(target_os = "windows"))]
impl DriveMountRegistry {
    fn new() -> Self {
        Self
    }
}

#[cfg(not(target_os = "windows"))]
impl SyncRootRegistry {
    fn new() -> Self {
        Self
    }
}

struct SqliteTransferStore {
    database: Database,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AppPreferences {
    start_minimized: bool,
}

fn preferences_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("preferences.json"))
        .map_err(|error| error.to_string())
}

fn load_preferences(path: &std::path::Path) -> Result<AppPreferences, String> {
    match fs::read(path) {
        Ok(contents) => serde_json::from_slice(&contents).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AppPreferences::default()),
        Err(error) => Err(error.to_string()),
    }
}

fn save_preferences(path: &std::path::Path, preferences: &AppPreferences) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Preferences path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    fs::write(
        temporary.path(),
        serde_json::to_vec_pretty(preferences).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error.to_string())
}

#[tauri::command]
fn app_start_minimized_get(app: tauri::AppHandle) -> Result<bool, String> {
    Ok(load_preferences(&preferences_path(&app)?)?.start_minimized)
}

#[tauri::command]
fn app_start_minimized_set(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let path = preferences_path(&app)?;
    let mut preferences = load_preferences(&path)?;
    preferences.start_minimized = enabled;
    save_preferences(&path, &preferences)
}

#[async_trait::async_trait]
impl TransferStore for SqliteTransferStore {
    async fn save(&self, transfer: TransferSnapshot) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO transfer_queue (id, connection_id, remote_path, direction, status, total_bytes, transferred_bytes, attempts, next_retry_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET status = excluded.status, total_bytes = excluded.total_bytes, transferred_bytes = excluded.transferred_bytes, attempts = excluded.attempts, next_retry_at = excluded.next_retry_at, updated_at = excluded.updated_at",
        )
        .bind(transfer.id.to_string())
        .bind(transfer.connection_id.to_string())
        .bind(transfer.path.as_str())
        .bind(match transfer.direction {
            TransferDirection::Download => "download",
            TransferDirection::Upload => "upload",
        })
        .bind(match transfer.status {
            TransferStatus::Pending => "pending",
            TransferStatus::Running => "running",
            TransferStatus::Paused => "paused",
            TransferStatus::Completed => "completed",
            TransferStatus::Failed => "failed",
            TransferStatus::Cancelled => "cancelled",
        })
        .bind(transfer.total_bytes.map(|value| value as i64))
        .bind(transfer.transferred_bytes as i64)
        .bind(transfer.attempts as i64)
        .bind(transfer.next_retry_at.map(system_time_string))
        .bind(&now)
        .bind(&now)
        .execute(self.database.pool())
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    async fn load_recoverable(&self) -> Result<Vec<TransferSnapshot>, String> {
        let rows = sqlx::query(
            "SELECT id, connection_id, remote_path, direction, status, total_bytes, transferred_bytes, attempts, next_retry_at FROM transfer_queue WHERE status IN ('pending', 'running', 'paused') ORDER BY created_at",
        )
        .fetch_all(self.database.pool())
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let id = uuid::Uuid::parse_str(&sqlx::Row::get::<String, _>(&row, "id"))
                    .map(bifrost_common::TransferId::from_uuid)
                    .map_err(|error| error.to_string())?;
                let connection_id =
                    uuid::Uuid::parse_str(&sqlx::Row::get::<String, _>(&row, "connection_id"))
                        .map(bifrost_common::ConnectionId::from_uuid)
                        .map_err(|error| error.to_string())?;
                let path = bifrost_common::RemotePath::parse(&sqlx::Row::get::<String, _>(
                    &row,
                    "remote_path",
                ))
                .map_err(|error| error.to_string())?;
                let direction = match sqlx::Row::get::<String, _>(&row, "direction").as_str() {
                    "download" => TransferDirection::Download,
                    "upload" => TransferDirection::Upload,
                    value => return Err(format!("unknown transfer direction: {value}")),
                };
                let status = match sqlx::Row::get::<String, _>(&row, "status").as_str() {
                    "pending" => TransferStatus::Pending,
                    "running" => TransferStatus::Running,
                    "paused" => TransferStatus::Paused,
                    value => return Err(format!("unknown recoverable transfer status: {value}")),
                };
                Ok(TransferSnapshot {
                    id,
                    connection_id,
                    path,
                    direction,
                    status,
                    total_bytes: sqlx::Row::get::<Option<i64>, _>(&row, "total_bytes")
                        .map(|value| value as u64),
                    transferred_bytes: sqlx::Row::get::<i64, _>(&row, "transferred_bytes") as u64,
                    attempts: sqlx::Row::get::<i64, _>(&row, "attempts") as u32,
                    next_retry_at: sqlx::Row::get::<Option<String>, _>(&row, "next_retry_at")
                        .and_then(system_time_parse),
                })
            })
            .collect()
    }

    async fn save_cache(&self, record: CacheRecord) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO cache_entries (connection_id, remote_path, local_path, size_bytes, last_accessed, pinned, active_transfer) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(connection_id, remote_path) DO UPDATE SET local_path = excluded.local_path, size_bytes = excluded.size_bytes, last_accessed = excluded.last_accessed, pinned = excluded.pinned, active_transfer = excluded.active_transfer",
        )
        .bind(record.connection_id.to_string())
        .bind(record.remote_path.as_str())
        .bind(record.local_path.to_string_lossy().as_ref())
        .bind(record.size_bytes as i64)
        .bind(system_time_string(record.last_accessed))
        .bind(record.pinned as i64)
        .bind(record.active_transfer as i64)
        .execute(self.database.pool())
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    async fn load_cache(&self) -> Result<Vec<CacheRecord>, String> {
        let rows = sqlx::query(
            "SELECT connection_id, remote_path, local_path, size_bytes, last_accessed, pinned, active_transfer FROM cache_entries",
        )
        .fetch_all(self.database.pool())
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let connection_id =
                    uuid::Uuid::parse_str(&sqlx::Row::get::<String, _>(&row, "connection_id"))
                        .map(bifrost_common::ConnectionId::from_uuid)
                        .map_err(|error| error.to_string())?;
                let remote_path = bifrost_common::RemotePath::parse(&sqlx::Row::get::<String, _>(
                    &row,
                    "remote_path",
                ))
                .map_err(|error| error.to_string())?;
                let last_accessed =
                    system_time_parse(sqlx::Row::get::<String, _>(&row, "last_accessed"))
                        .ok_or_else(|| "invalid cache last_accessed timestamp".to_owned())?;
                Ok(CacheRecord {
                    connection_id,
                    remote_path,
                    local_path: PathBuf::from(sqlx::Row::get::<String, _>(&row, "local_path")),
                    size_bytes: sqlx::Row::get::<i64, _>(&row, "size_bytes") as u64,
                    last_accessed,
                    pinned: sqlx::Row::get::<i64, _>(&row, "pinned") != 0,
                    active_transfer: sqlx::Row::get::<i64, _>(&row, "active_transfer") != 0,
                })
            })
            .collect()
    }
}

fn system_time_string(value: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(value).to_rfc3339()
}

fn system_time_parse(value: String) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc).into())
}

fn default_sync_root_path(connection: &ConnectionRecord) -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let safe_name: String = connection
        .name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let safe_name = safe_name.trim().trim_matches('.');
    let name = if safe_name.is_empty() {
        "connection"
    } else {
        safe_name
    };
    home.join("Bifrost Drive")
        .join(format!("{}-{}", name, connection.id))
}

fn normalize_drive_letter(value: Option<&str>) -> Result<Option<char>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value = value.strip_suffix(':').unwrap_or(value);
    let mut characters = value.chars();
    let letter = characters
        .next()
        .filter(char::is_ascii_alphabetic)
        .ok_or_else(|| "Drive letter must be a single letter from D to Z".to_owned())?
        .to_ascii_uppercase();
    if characters.next().is_some() || !('D'..='Z').contains(&letter) {
        return Err("Drive letter must be a single letter from D to Z".to_owned());
    }
    Ok(Some(letter))
}

fn set_drive_letter(
    configuration: &mut serde_json::Value,
    value: Option<&str>,
) -> Result<(), String> {
    let object = configuration
        .as_object_mut()
        .ok_or_else(|| "Connection configuration must be an object".to_owned())?;
    match normalize_drive_letter(value)? {
        Some(letter) => {
            object.insert(
                "drive_letter".to_owned(),
                serde_json::Value::String(letter.to_string()),
            );
        }
        None => {
            object.remove("drive_letter");
        }
    }
    Ok(())
}

fn configured_drive_letter(configuration_json: &str) -> Result<Option<char>, String> {
    let configuration: serde_json::Value = serde_json::from_str(configuration_json)
        .map_err(|_| "Connection configuration is invalid".to_owned())?;
    normalize_drive_letter(
        configuration
            .get("drive_letter")
            .and_then(serde_json::Value::as_str),
    )
}

fn set_mount_on_startup(
    configuration: &mut serde_json::Value,
    enabled: bool,
) -> Result<(), String> {
    configuration
        .as_object_mut()
        .ok_or_else(|| "Connection configuration must be an object".to_owned())?
        .insert(
            "mount_on_startup".to_owned(),
            serde_json::Value::Bool(enabled),
        );
    Ok(())
}

fn set_drive_presentation(
    configuration: &mut serde_json::Value,
    drive_type: &str,
    drive_icon: Option<&str>,
) -> Result<(), String> {
    let drive_type = if drive_type.trim().is_empty() {
        "network"
    } else {
        drive_type.trim()
    };
    if !matches!(drive_type, "network" | "local") {
        return Err("Drive type must be network or local".to_owned());
    }
    let drive_icon = drive_icon
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("system");
    if !matches!(
        drive_icon,
        "system" | "bifrost" | "windows_local" | "windows_network"
    ) && drive_icon
        .strip_prefix("stock:")
        .is_none_or(|id| id.parse::<i32>().is_err())
        && drive_icon
            .strip_prefix("shell32:")
            .is_none_or(|index| index.parse::<i32>().is_err())
    {
        let path = std::path::Path::new(drive_icon);
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "ico" | "exe" | "dll" | "png" | "jpg" | "jpeg" | "webp"
                )
            });
        if !path.is_absolute() || !path.is_file() || !supported {
            return Err(
                "Custom drive icon must be an existing .ico, .exe, .dll, .png, .jpg, or .webp file"
                    .to_owned(),
            );
        }
    }
    let object = configuration
        .as_object_mut()
        .ok_or_else(|| "Connection configuration must be an object".to_owned())?;
    object.insert("drive_type".to_owned(), serde_json::json!(drive_type));
    object.insert("drive_icon".to_owned(), serde_json::json!(drive_icon));
    Ok(())
}

fn configured_drive_type(configuration_json: &str) -> Result<bool, String> {
    let configuration: serde_json::Value = serde_json::from_str(configuration_json)
        .map_err(|_| "Connection configuration is invalid".to_owned())?;
    Ok(configuration
        .get("drive_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("network")
        != "local")
}

fn convert_drive_image_to_ico(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    let image = image::open(source)
        .map_err(|error| error.to_string())?
        .resize_to_fill(256, 256, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let icon = ico::IconImage::from_rgba_data(256, 256, image.into_raw());
    let entry = ico::IconDirEntry::encode(&icon).map_err(|error| error.to_string())?;
    let mut directory = ico::IconDir::new(ico::ResourceType::Icon);
    directory.add_entry(entry);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = std::fs::File::create(destination).map_err(|error| error.to_string())?;
    directory.write(file).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn configured_drive_icon(
    configuration_json: &str,
    data_dir: &std::path::Path,
    connection_id: bifrost_common::ConnectionId,
) -> Result<Option<String>, String> {
    let configuration: serde_json::Value = serde_json::from_str(configuration_json)
        .map_err(|_| "Connection configuration is invalid".to_owned())?;
    let icon = configuration
        .get("drive_icon")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("system");
    let windows_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let source = match icon {
        "system" => None,
        "bifrost" => Some(format!(
            "\"{}\",0",
            std::env::current_exe()
                .map_err(|error| error.to_string())?
                .display()
        )),
        "windows_local" => Some(format!(r"{windows_root}\System32\shell32.dll,8")),
        "windows_network" => Some(format!(r"{windows_root}\System32\shell32.dll,9")),
        stock if stock.starts_with("stock:") => {
            let id = stock
                .trim_start_matches("stock:")
                .parse::<i32>()
                .map_err(|_| "Invalid Windows stock icon".to_owned())?;
            Some(stock_icon_source(id)?)
        }
        shell if shell.starts_with("shell32:") => {
            let index = shell
                .trim_start_matches("shell32:")
                .parse::<i32>()
                .map_err(|_| "Invalid shell32 icon index".to_owned())?;
            let path = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
            Some(format!(r#""{path}\System32\SHELL32.dll",{index}"#))
        }
        path => {
            let path = std::path::Path::new(path);
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                let destination = data_dir
                    .join("drive-icons")
                    .join(format!("{connection_id}.ico"));
                convert_drive_image_to_ico(path, &destination)?;
                Some(format!("\"{}\",0", destination.display()))
            } else {
                Some(format!("\"{}\",0", path.display()))
            }
        }
    };
    Ok(source)
}

#[cfg(target_os = "windows")]
const STOCK_DRIVE_ICONS: &[(i32, &str)] = &[
    (8, "Fixed drive"),
    (9, "Network drive"),
    (10, "Disconnected network"),
    (7, "Removable drive"),
    (11, "CD drive"),
    (59, "DVD drive"),
    (12, "RAM drive"),
    (15, "Server"),
    (51, "Shared server"),
    (3, "Folder"),
    (4, "Open folder"),
    (17, "Network"),
    (94, "Computer"),
    (13, "World"),
];

#[cfg(target_os = "windows")]
fn windows_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Clone, serde::Serialize)]
struct StockDriveIcon {
    value: String,
    label: String,
    preview: String,
}

#[cfg(target_os = "windows")]
fn stock_icon_info(
    id: i32,
    flags: windows::Win32::UI::Shell::SHGSI_FLAGS,
) -> Result<windows::Win32::UI::Shell::SHSTOCKICONINFO, String> {
    use windows::Win32::UI::Shell::{SHGetStockIconInfo, SHSTOCKICONID, SHSTOCKICONINFO};
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe { SHGetStockIconInfo(SHSTOCKICONID(id), flags, &mut info) }
        .map_err(|error| error.to_string())?;
    Ok(info)
}

#[cfg(target_os = "windows")]
fn stock_icon_source(id: i32) -> Result<String, String> {
    use windows::Win32::UI::Shell::SHGSI_ICONLOCATION;
    let info = stock_icon_info(id, SHGSI_ICONLOCATION)?;
    let length = info
        .szPath
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(info.szPath.len());
    let path = String::from_utf16(&info.szPath[..length]).map_err(|error| error.to_string())?;
    Ok(format!("\"{path}\",{}", info.iIcon))
}

#[cfg(target_os = "windows")]
fn stock_icon_preview(id: i32) -> Result<String, String> {
    use windows::Win32::UI::Shell::{SHGSI_ICON, SHGSI_LARGEICON};
    let info = stock_icon_info(id, SHGSI_ICON | SHGSI_LARGEICON)?;
    icon_handle_preview(info.hIcon)
}

#[cfg(target_os = "windows")]
fn icon_handle_preview(
    icon_handle: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> Result<String, String> {
    use windows::Win32::{
        Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        },
        UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO},
    };
    let mut icon_info = ICONINFO::default();
    unsafe { GetIconInfo(icon_handle, &mut icon_info) }.map_err(|error| error.to_string())?;
    let mut bitmap = BITMAP::default();
    let result = unsafe {
        GetObjectW(
            icon_info.hbmColor.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut bitmap as *mut BITMAP).cast()),
        )
    };
    if result == 0 {
        unsafe {
            let _ = DeleteObject(icon_info.hbmColor.into());
            let _ = DeleteObject(icon_info.hbmMask.into());
            let _ = DestroyIcon(icon_handle);
        }
        return Err("Could not inspect Windows stock icon".to_owned());
    }
    let width = bitmap.bmWidth as u32;
    let height = bitmap.bmHeight as u32;
    let mut bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let device = unsafe { GetDC(None) };
    let lines = unsafe {
        GetDIBits(
            device,
            icon_info.hbmColor,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        let _ = ReleaseDC(None, device);
        let _ = DeleteObject(icon_info.hbmColor.into());
        let _ = DeleteObject(icon_info.hbmMask.into());
        let _ = DestroyIcon(icon_handle);
    }
    if lines == 0 {
        return Err("Could not render Windows stock icon".to_owned());
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    rgba_preview(width, height, &pixels)
}

#[cfg(target_os = "windows")]
fn rgba_preview(width: u32, height: u32, pixels: &[u8]) -> Result<String, String> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

#[cfg(target_os = "windows")]
fn shell_icon_preview(path: &std::path::Path, index: i32) -> Result<String, String> {
    use windows::{
        core::PCWSTR,
        Win32::UI::{Shell::ExtractIconExW, WindowsAndMessaging::HICON},
    };
    let path = windows_wide_null(&path.to_string_lossy());
    let mut icon = HICON::default();
    let extracted =
        unsafe { ExtractIconExW(PCWSTR(path.as_ptr()), index, Some(&mut icon), None, 1) };
    if extracted == 0 || icon.is_invalid() {
        return Err("The selected file does not contain a usable icon".to_owned());
    }
    icon_handle_preview(icon)
}

#[cfg(target_os = "windows")]
fn drive_icon_file_preview(path: &std::path::Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "webp" => {
            let image = image::open(path)
                .map_err(|error| error.to_string())?
                .resize_to_fill(64, 64, image::imageops::FilterType::Lanczos3)
                .to_rgba8();
            rgba_preview(64, 64, &image)
        }
        "ico" => {
            let directory =
                ico::IconDir::read(std::fs::File::open(path).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
            let entry = directory
                .entries()
                .iter()
                .max_by_key(|entry| entry.width().saturating_mul(entry.height()))
                .ok_or_else(|| "The selected ICO file is empty".to_owned())?;
            let image = entry.decode().map_err(|error| error.to_string())?;
            rgba_preview(image.width(), image.height(), image.rgba_data())
        }
        "exe" | "dll" => shell_icon_preview(path, 0),
        _ => Err("Unsupported drive icon file".to_owned()),
    }
}

#[tauri::command]
fn drive_icon_preview(request: DriveIconPreviewRequest) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = request;
        Err("Drive icon previews are available only on Windows".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        drive_icon_file_preview(std::path::Path::new(&request.path))
    }
}

#[cfg(target_os = "windows")]
fn configured_drive_icon_preview(configuration_json: &str) -> Option<String> {
    let configuration: serde_json::Value = serde_json::from_str(configuration_json).ok()?;
    let icon = configuration.get("drive_icon")?.as_str()?;
    match icon {
        "system" => None,
        "bifrost" => shell_icon_preview(&std::env::current_exe().ok()?, 0).ok(),
        "windows_local" => stock_icon_preview(8).ok(),
        "windows_network" => stock_icon_preview(9).ok(),
        stock if stock.starts_with("stock:") => stock
            .trim_start_matches("stock:")
            .parse::<i32>()
            .ok()
            .and_then(|id| stock_icon_preview(id).ok()),
        shell if shell.starts_with("shell32:") => {
            let index = shell.trim_start_matches("shell32:").parse::<i32>().ok()?;
            let root = std::env::var("SystemRoot").ok()?;
            shell_icon_preview(
                &std::path::Path::new(&root)
                    .join("System32")
                    .join("SHELL32.dll"),
                index,
            )
            .ok()
        }
        path => drive_icon_file_preview(std::path::Path::new(path)).ok(),
    }
}

#[cfg(target_os = "windows")]
fn shell32_icons() -> Result<Vec<StockDriveIcon>, String> {
    use windows::{
        core::PCWSTR,
        Win32::UI::{Shell::ExtractIconExW, WindowsAndMessaging::HICON},
    };
    let windows_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let shell32 = format!(r"{windows_root}\System32\SHELL32.dll");
    let shell32_wide = windows_wide_null(&shell32);
    let count = unsafe { ExtractIconExW(PCWSTR(shell32_wide.as_ptr()), -1, None, None, 0) };
    let mut icons = Vec::with_capacity(count as usize);
    for index in 0..count as i32 {
        let mut icon = HICON::default();
        let extracted = unsafe {
            ExtractIconExW(
                PCWSTR(shell32_wide.as_ptr()),
                index,
                Some(&mut icon),
                None,
                1,
            )
        };
        if extracted == 0 || icon.is_invalid() {
            continue;
        }
        if let Ok(preview) = icon_handle_preview(icon) {
            icons.push(StockDriveIcon {
                value: format!("shell32:{index}"),
                label: format!("Icon {index}"),
                preview,
            });
        }
    }
    Ok(icons)
}

#[tauri::command]
fn drive_stock_icons() -> Result<Vec<StockDriveIcon>, String> {
    #[cfg(not(target_os = "windows"))]
    {
        Ok(Vec::new())
    }
    #[cfg(target_os = "windows")]
    {
        static ICONS: std::sync::OnceLock<Vec<StockDriveIcon>> = std::sync::OnceLock::new();
        if let Some(icons) = ICONS.get() {
            return Ok(icons.clone());
        }
        let mut icons = STOCK_DRIVE_ICONS
            .iter()
            .map(|(id, label)| {
                Ok(StockDriveIcon {
                    value: format!("stock:{id}"),
                    label: (*label).to_owned(),
                    preview: stock_icon_preview(*id)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        icons.extend(shell32_icons()?);
        let _ = ICONS.set(icons.clone());
        Ok(icons)
    }
}

fn connection_activity_details(connection: &ConnectionRecord) -> String {
    let drive = configured_drive_letter(&connection.configuration_json)
        .ok()
        .flatten()
        .map(|letter| format!(" · Drive {letter}:"))
        .unwrap_or_default();
    format!(
        "{} · {} · {}{}",
        connection.name,
        connection.kind.as_str(),
        connection.endpoint,
        drive
    )
}

async fn record_connection_activity(
    database: &Database,
    kind: &str,
    connection: &ConnectionRecord,
) {
    if let Err(error) = database
        .insert_activity(
            kind,
            Some(&connection_activity_details(connection)),
            "completed",
        )
        .await
    {
        tracing::warn!(%error, %kind, "activity history write failed");
    }
}

async fn ensure_drive_letter_unassigned(
    database: &Database,
    requested: Option<&str>,
    excluding: Option<bifrost_common::ConnectionId>,
) -> Result<(), String> {
    let Some(requested) = normalize_drive_letter(requested)? else {
        return Ok(());
    };
    for connection in database
        .list_connections()
        .await
        .map_err(|error| error.to_string())?
    {
        if excluding == Some(connection.id) {
            continue;
        }
        if configured_drive_letter(&connection.configuration_json)? == Some(requested) {
            return Err(format!(
                "Drive {requested}: is already assigned to {}",
                connection.name
            ));
        }
    }
    Ok(())
}

#[tauri::command]
fn app_status(application: State<'_, Mutex<Application>>) -> AppStatus {
    application
        .lock()
        .expect("application state poisoned")
        .status()
}

#[tauri::command]
async fn connections_list(database: State<'_, Database>) -> Result<Vec<ConnectionSummary>, String> {
    database
        .list_connections()
        .await
        .map(|connections| {
            connections
                .into_iter()
                .map(|connection| ConnectionSummary {
                    id: connection.id,
                    name: connection.name,
                    kind: connection.kind,
                    state: ConnectionState::Disconnected,
                    endpoint: connection.endpoint,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn connections_details(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: ConnectionIdRequest,
) -> Result<bifrost_api::ConnectionDetails, String> {
    let connection = database
        .find_connection(request.id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let credential: CredentialRef = serde_json::from_str(&connection.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    let username = match connection.kind {
        ProviderKind::Sftp => {
            serde_json::from_str::<SftpCredentials>(secret.expose())
                .map_err(|_| "Stored SFTP credential payload is invalid".to_owned())?
                .username
        }
        ProviderKind::WebDav | ProviderKind::Nextcloud | ProviderKind::Ftp | ProviderKind::Smb => {
            serde_json::from_str::<WebDavCredentials>(secret.expose())
                .map_err(|_| "Stored credential payload is invalid".to_owned())?
                .username
        }
        ProviderKind::S3 => String::new(),
    };
    #[cfg(target_os = "windows")]
    let drive_icon_preview = configured_drive_icon_preview(&connection.configuration_json);
    #[cfg(not(target_os = "windows"))]
    let drive_icon_preview = None;
    Ok(bifrost_api::ConnectionDetails {
        summary: ConnectionSummary {
            id: connection.id,
            name: connection.name,
            kind: connection.kind,
            state: ConnectionState::Disconnected,
            endpoint: connection.endpoint,
        },
        configuration: serde_json::from_str(&connection.configuration_json)
            .map_err(|_| "Connection configuration is invalid".to_owned())?,
        username: (!username.is_empty()).then_some(username),
        drive_icon_preview,
    })
}

fn merge_connection_credentials(
    existing: &str,
    updates: serde_json::Value,
) -> Result<String, String> {
    let mut merged = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(existing)
        .map_err(|_| "Stored credential payload is invalid".to_owned())?;
    let updates = updates
        .as_object()
        .ok_or_else(|| "Connection credentials are invalid".to_owned())?;
    for (key, value) in updates {
        let blank = value.as_str().is_some_and(str::is_empty);
        if !value.is_null() && !blank {
            merged.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_string(&merged).map_err(|_| "Connection credentials are invalid".to_owned())
}

#[tauri::command]
async fn connections_update(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    mounts: State<'_, DriveMountRegistry>,
    mut request: bifrost_api::UpdateConnectionRequest,
) -> Result<ConnectionSummary, String> {
    if request.name.trim().is_empty() {
        return Err("Connection name is required".to_owned());
    }
    let existing = database
        .find_connection(request.id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let requested_drive = request
        .configuration
        .get("drive_letter")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    ensure_drive_letter_unassigned(&database, requested_drive.as_deref(), Some(existing.id))
        .await?;
    set_drive_letter(&mut request.configuration, requested_drive.as_deref())?;
    let drive_type = request
        .configuration
        .get("drive_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("network")
        .to_owned();
    let drive_icon = request
        .configuration
        .get("drive_icon")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    set_drive_presentation(
        &mut request.configuration,
        &drive_type,
        drive_icon.as_deref(),
    )?;
    let old_credential: CredentialRef = serde_json::from_str(&existing.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let old_secret = credentials
        .get(&old_credential)
        .await
        .map_err(|error| error.to_string())?;
    let merged_secret = merge_connection_credentials(old_secret.expose(), request.credentials)?;
    let new_credential = credentials
        .put(
            "connection",
            &request.name,
            SecretString::new(merged_secret),
        )
        .await
        .map_err(|error| error.to_string())?;
    let new_credential_ref = serde_json::to_string(&new_credential)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    if let Err(error) = test_connection(
        &credentials,
        TestConnectionRequest {
            kind: existing.kind,
            endpoint: request.endpoint.clone(),
            credential_ref: new_credential_ref.clone(),
            configuration: request.configuration.clone(),
        },
    )
    .await
    {
        let _ = credentials.delete(&new_credential).await;
        return Err(error);
    }
    let updated = ConnectionRecord {
        id: existing.id,
        name: request.name,
        kind: existing.kind,
        endpoint: request.endpoint,
        credential_ref: new_credential_ref,
        configuration_json: request.configuration.to_string(),
    };
    if let Err(error) = database.update_connection(&updated).await {
        let _ = credentials.delete(&new_credential).await;
        return Err(error.to_string());
    }
    record_connection_activity(&database, "connection_updated", &updated).await;
    let _ = credentials.delete(&old_credential).await;
    #[cfg(target_os = "windows")]
    drop(mounts.take(&updated.id.to_string()));
    #[cfg(target_os = "windows")]
    if !configured_drive_type(&updated.configuration_json)? {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if let Err(error) = cleanup_bifrost_mount_points_for_share(&existing.name) {
            tracing::warn!(%error, "could not remove stale network metadata after local conversion");
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = mounts;
    Ok(ConnectionSummary {
        id: updated.id,
        name: updated.name,
        kind: updated.kind,
        state: ConnectionState::Connected,
        endpoint: updated.endpoint,
    })
}

#[tauri::command]
async fn activity_list(database: State<'_, Database>) -> Result<Vec<ActivitySummary>, String> {
    database
        .list_activity()
        .await
        .map(|events| {
            events
                .into_iter()
                .map(|event| ActivitySummary {
                    id: event.id,
                    kind: event.kind,
                    remote_path: event.remote_path,
                    status: event.status,
                    created_at: event.created_at,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn connections_create(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    mut request: CreateConnectionRequest,
) -> Result<ConnectionSummary, String> {
    if request.name.trim().is_empty() || request.credential_ref.trim().is_empty() {
        return Err("Connection name and credential reference are required".to_owned());
    }
    let requested_drive = request
        .configuration
        .get("drive_letter")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    ensure_drive_letter_unassigned(&database, requested_drive.as_deref(), None).await?;
    set_drive_letter(&mut request.configuration, requested_drive.as_deref())?;
    let drive_type = request
        .configuration
        .get("drive_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("network")
        .to_owned();
    let drive_icon = request
        .configuration
        .get("drive_icon")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    set_drive_presentation(
        &mut request.configuration,
        &drive_type,
        drive_icon.as_deref(),
    )?;
    let credential: CredentialRef = serde_json::from_str(&request.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let test_request = TestConnectionRequest {
        kind: request.kind,
        endpoint: request.endpoint.clone(),
        credential_ref: request.credential_ref.clone(),
        configuration: request.configuration.clone(),
    };
    if let Err(error) = test_connection(&credentials, test_request).await {
        let _ = credentials.delete(&credential).await;
        return Err(error);
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
    }

    let record = ConnectionRecord {
        id: bifrost_common::ConnectionId::new(),
        name: request.name,
        kind: request.kind,
        endpoint: endpoint.to_string(),
        credential_ref: serde_json::to_string(&credential)
            .map_err(|_| "Connection credential reference is invalid".to_owned())?,
        configuration_json: request.configuration.to_string(),
    };
    database
        .insert_connection(&record)
        .await
        .map_err(|error| error.to_string())?;
    record_connection_activity(&database, "connection_added", &record).await;
    Ok(ConnectionSummary {
        id: record.id,
        name: record.name,
        kind: record.kind,
        state: ConnectionState::Disconnected,
        endpoint: record.endpoint,
    })
}

#[tauri::command]
async fn connections_remove(
    registry: State<'_, SyncRootRegistry>,
    mounts: State<'_, DriveMountRegistry>,
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: ConnectionIdRequest,
) -> Result<(), String> {
    let connection = database
        .find_connection(request.id)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "windows")]
    {
        drop(mounts.take(&request.id.to_string()));
        drop(registry.take(&request.id.to_string()));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (&registry, &mounts);
    let Some(connection) = connection else {
        return Ok(());
    };
    let credential: CredentialRef = serde_json::from_str(&connection.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    database
        .delete_connection(request.id)
        .await
        .map_err(|error| error.to_string())?;
    record_connection_activity(&database, "connection_removed", &connection).await;
    if let Err(error) = credentials.delete(&credential).await {
        if !matches!(error, CredentialError::NotFound) {
            tracing::warn!(%error, "connection removed but credential cleanup failed");
        }
    }
    Ok(())
}

#[tauri::command]
async fn credentials_store_s3(
    credentials: State<'_, WindowsCredentialStore>,
    request: StoreS3CredentialRequest,
) -> Result<CredentialSummary, String> {
    if request.label.trim().is_empty() {
        return Err("Credential label is required".to_owned());
    }
    if request.access_key_id.is_empty() || request.secret_access_key.is_empty() {
        return Err("S3 access key and secret key are required".to_owned());
    }
    let payload = serde_json::json!({
        "access_key_id": request.access_key_id,
        "secret_access_key": request.secret_access_key,
    });
    let credential = credentials
        .put("s3", &request.label, SecretString::new(payload.to_string()))
        .await
        .map_err(|error| error.to_string())?;
    Ok(CredentialSummary {
        id: credential.id,
        kind: credential.kind,
        label: credential.label,
    })
}

#[tauri::command]
async fn connections_create_s3(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateS3ConnectionRequest,
) -> Result<ConnectionSummary, String> {
    ensure_drive_letter_unassigned(&database, request.drive_letter.as_deref(), None).await?;
    if request.name.trim().is_empty()
        || request.region.trim().is_empty()
        || request.bucket.trim().is_empty()
    {
        return Err("Connection name, bucket, and region are required".to_owned());
    }
    if request.access_key_id.is_empty() || request.secret_access_key.is_empty() {
        return Err("S3 access key and secret key are required".to_owned());
    }
    let credential = credentials_store_s3(
        credentials.clone(),
        StoreS3CredentialRequest {
            label: request.name.clone(),
            access_key_id: request.access_key_id.clone(),
            secret_access_key: request.secret_access_key.clone(),
        },
    )
    .await?;
    let credential_ref = serde_json::to_string(&credential)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let mut configuration = serde_json::json!({
        "region": request.region,
        "bucket": request.bucket,
        "path_style": request.path_style,
    });
    set_drive_letter(&mut configuration, request.drive_letter.as_deref())?;
    set_mount_on_startup(&mut configuration, request.mount_on_startup)?;
    set_drive_presentation(
        &mut configuration,
        &request.drive_type,
        request.drive_icon.as_deref(),
    )?;
    let test_request = TestConnectionRequest {
        kind: ProviderKind::S3,
        endpoint: request.endpoint.clone(),
        credential_ref: credential_ref.clone(),
        configuration: configuration.clone(),
    };
    if let Err(error) = test_connection(&credentials, test_request).await {
        let _ = credentials
            .delete(&CredentialRef {
                id: credential.id,
                kind: credential.kind,
                label: credential.label,
            })
            .await;
        return Err(error);
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
    }
    let record = ConnectionRecord {
        id: bifrost_common::ConnectionId::new(),
        name: request.name,
        kind: ProviderKind::S3,
        endpoint: endpoint.to_string(),
        credential_ref,
        configuration_json: configuration.to_string(),
    };
    if let Err(error) = database.insert_connection(&record).await {
        let _ = credentials
            .delete(&CredentialRef {
                id: credential.id,
                kind: credential.kind,
                label: credential.label,
            })
            .await;
        return Err(error.to_string());
    }
    record_connection_activity(&database, "connection_added", &record).await;
    Ok(ConnectionSummary {
        id: record.id,
        name: record.name,
        kind: record.kind,
        state: ConnectionState::Connected,
        endpoint: record.endpoint,
    })
}

#[derive(Debug, Deserialize)]
struct StoredS3Credentials {
    access_key_id: String,
    secret_access_key: String,
}

#[derive(Debug, Deserialize)]
struct S3ConnectionConfiguration {
    region: String,
    bucket: String,
    path_style: bool,
}

#[derive(Debug, Deserialize)]
struct WebDavCredentials {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct SftpConfiguration {
    host: String,
    port: u16,
    #[serde(default)]
    root_path: String,
    #[serde(default = "default_sftp_known_hosts")]
    known_hosts: String,
    #[serde(default)]
    trust_on_first_use: bool,
    #[serde(default = "default_sftp_authentication")]
    authentication: String,
    private_key_path: Option<String>,
}

fn default_sftp_authentication() -> String {
    "password".to_owned()
}

fn default_sftp_known_hosts() -> String {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|path| path.join(".ssh").join("known_hosts").display().to_string())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct SftpCredentials {
    username: String,
    password: Option<String>,
    private_key_path: Option<String>,
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SmbConfiguration {
    domain: String,
}

async fn test_connection(
    credentials: &WindowsCredentialStore,
    request: TestConnectionRequest,
) -> Result<(), String> {
    let credential: CredentialRef = serde_json::from_str(&request.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    match request.kind {
        ProviderKind::S3 => {
            let endpoint = url::Url::parse(&request.endpoint)
                .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
            if !matches!(endpoint.scheme(), "http" | "https") {
                return Err("Connection endpoint must use HTTP or HTTPS".to_owned());
            }
            let stored: StoredS3Credentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored S3 credential payload is invalid".to_owned())?;
            let configuration: S3ConnectionConfiguration =
                serde_json::from_value(request.configuration)
                    .map_err(|_| "S3 bucket configuration is invalid".to_owned())?;
            if configuration.bucket.trim().is_empty() || configuration.region.trim().is_empty() {
                return Err("S3 bucket and region are required".to_owned());
            }
            S3Provider::connect(
                S3Config {
                    endpoint,
                    region: configuration.region,
                    bucket: configuration.bucket,
                    path_style: configuration.path_style,
                },
                stored.access_key_id,
                stored.secret_access_key,
            )
            .await
            .map_err(|error| error.to_string())?
            .test_connection()
            .await
            .map_err(|error| error.to_string())
        }
        ProviderKind::WebDav | ProviderKind::Nextcloud => {
            let endpoint = url::Url::parse(&request.endpoint)
                .map_err(|_| "WebDAV endpoint must be a valid URL".to_owned())?;
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored WebDAV credential payload is invalid".to_owned())?;
            WebDavProvider::connect(
                WebDavConfig {
                    endpoint,
                    username: stored.username,
                },
                stored.password,
            )
            .map_err(|error| error.to_string())?
            .test_connection()
            .await
            .map_err(|error| error.to_string())
        }
        ProviderKind::Sftp => {
            let configuration: SftpConfiguration = serde_json::from_value(request.configuration)
                .map_err(|_| "SFTP configuration is invalid".to_owned())?;
            let stored: SftpCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored SFTP credential payload is invalid".to_owned())?;
            let config = SftpConfig {
                host: configuration.host,
                port: configuration.port,
                username: stored.username,
                known_hosts: configuration.known_hosts.into(),
                root_path: configuration.root_path,
                trust_on_first_use: configuration.trust_on_first_use,
            };
            let provider = (if configuration.authentication == "private_key" {
                let key_path = configuration
                    .private_key_path
                    .or(stored.private_key_path)
                    .ok_or_else(|| "SFTP private key path is required".to_owned())?;
                SftpProvider::connect_with_private_key(config, key_path.into(), stored.passphrase)
            } else {
                SftpProvider::connect(
                    config,
                    stored
                        .password
                        .ok_or_else(|| "SFTP password is required".to_owned())?,
                )
            })
            .map_err(|error| error.to_string())?;
            provider
                .test_connection()
                .await
                .map_err(|error| error.to_string())
        }
        ProviderKind::Ftp => {
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored FTP credential payload is invalid".to_owned())?;
            let endpoint = url::Url::parse(&request.endpoint)
                .map_err(|_| "FTP endpoint must be a valid URL".to_owned())?;
            FtpProvider::connect(FtpConfig {
                endpoint,
                username: stored.username,
                password: stored.password,
            })
            .map_err(|error| error.to_string())?
            .test_connection()
            .await
            .map_err(|error| error.to_string())
        }
        ProviderKind::Smb => {
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored SMB credential payload is invalid".to_owned())?;
            let configuration: SmbConfiguration = serde_json::from_value(request.configuration)
                .map_err(|_| "SMB configuration is invalid".to_owned())?;
            let endpoint = url::Url::parse(&request.endpoint)
                .map_err(|_| "SMB endpoint must be a valid URL".to_owned())?;
            SmbProvider::connect(SmbConfig {
                endpoint,
                username: stored.username,
                password: stored.password,
                domain: configuration.domain,
            })
            .await
            .map_err(|error| error.to_string())?
            .test_connection()
            .await
            .map_err(|error| error.to_string())
        }
    }
}

async fn create_tested_connection(
    database: &Database,
    credentials: &WindowsCredentialStore,
    name: String,
    kind: ProviderKind,
    endpoint: String,
    configuration: serde_json::Value,
    secret_payload: serde_json::Value,
) -> Result<ConnectionSummary, String> {
    let credential = credentials
        .put(
            "connection",
            &name,
            SecretString::new(secret_payload.to_string()),
        )
        .await
        .map_err(|error| error.to_string())?;
    let credential_ref = serde_json::to_string(&credential)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    if let Err(error) = test_connection(
        credentials,
        TestConnectionRequest {
            kind,
            endpoint: endpoint.clone(),
            credential_ref: credential_ref.clone(),
            configuration: configuration.clone(),
        },
    )
    .await
    {
        let _ = credentials.delete(&credential).await;
        return Err(error);
    }
    let record = ConnectionRecord {
        id: bifrost_common::ConnectionId::new(),
        name,
        kind,
        endpoint,
        credential_ref,
        configuration_json: configuration.to_string(),
    };
    if let Err(error) = database.insert_connection(&record).await {
        let _ = credentials.delete(&credential).await;
        return Err(error.to_string());
    }
    record_connection_activity(database, "connection_added", &record).await;
    Ok(ConnectionSummary {
        id: record.id,
        name: record.name,
        kind: record.kind,
        state: ConnectionState::Connected,
        endpoint: record.endpoint,
    })
}

async fn provider_for_connection(
    connection: &ConnectionRecord,
    credentials: &WindowsCredentialStore,
) -> Result<Box<dyn StorageProvider>, String> {
    let credential: CredentialRef = serde_json::from_str(&connection.credential_ref)
        .map_err(|_| "Connection credential reference is invalid".to_owned())?;
    let secret = credentials
        .get(&credential)
        .await
        .map_err(|error| error.to_string())?;
    match connection.kind {
        ProviderKind::S3 => {
            let stored: StoredS3Credentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored S3 credential payload is invalid".to_owned())?;
            let configuration: S3ConnectionConfiguration =
                serde_json::from_str(&connection.configuration_json)
                    .map_err(|_| "S3 bucket configuration is invalid".to_owned())?;
            let endpoint = url::Url::parse(&connection.endpoint)
                .map_err(|_| "Connection endpoint must be a valid URL".to_owned())?;
            Ok(Box::new(
                S3Provider::connect(
                    S3Config {
                        endpoint,
                        region: configuration.region,
                        bucket: configuration.bucket,
                        path_style: configuration.path_style,
                    },
                    stored.access_key_id,
                    stored.secret_access_key,
                )
                .await
                .map_err(|error| error.to_string())?,
            ))
        }
        ProviderKind::WebDav | ProviderKind::Nextcloud => {
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored WebDAV credential payload is invalid".to_owned())?;
            let endpoint = url::Url::parse(&connection.endpoint)
                .map_err(|_| "WebDAV endpoint must be a valid URL".to_owned())?;
            Ok(Box::new(
                WebDavProvider::connect(
                    WebDavConfig {
                        endpoint,
                        username: stored.username,
                    },
                    stored.password,
                )
                .map_err(|error| error.to_string())?,
            ))
        }
        ProviderKind::Sftp => {
            let configuration: SftpConfiguration =
                serde_json::from_str(&connection.configuration_json)
                    .map_err(|_| "SFTP configuration is invalid".to_owned())?;
            let stored: SftpCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored SFTP credential payload is invalid".to_owned())?;
            let config = SftpConfig {
                host: configuration.host,
                port: configuration.port,
                username: stored.username,
                known_hosts: configuration.known_hosts.into(),
                root_path: configuration.root_path,
                trust_on_first_use: configuration.trust_on_first_use,
            };
            if configuration.authentication == "private_key" {
                let key_path = configuration
                    .private_key_path
                    .or(stored.private_key_path)
                    .ok_or_else(|| "SFTP private key path is required".to_owned())?;
                Ok(Box::new(
                    SftpProvider::connect_with_private_key(
                        config,
                        key_path.into(),
                        stored.passphrase,
                    )
                    .map_err(|error| error.to_string())?,
                ))
            } else {
                Ok(Box::new(
                    SftpProvider::connect(
                        config,
                        stored
                            .password
                            .ok_or_else(|| "SFTP password is required".to_owned())?,
                    )
                    .map_err(|error| error.to_string())?,
                ))
            }
        }
        ProviderKind::Ftp => {
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored FTP credential payload is invalid".to_owned())?;
            let endpoint = url::Url::parse(&connection.endpoint)
                .map_err(|_| "FTP endpoint must be a valid URL".to_owned())?;
            Ok(Box::new(
                FtpProvider::connect(FtpConfig {
                    endpoint,
                    username: stored.username,
                    password: stored.password,
                })
                .map_err(|error| error.to_string())?,
            ))
        }
        ProviderKind::Smb => {
            let stored: WebDavCredentials = serde_json::from_str(secret.expose())
                .map_err(|_| "Stored SMB credential payload is invalid".to_owned())?;
            let configuration: SmbConfiguration =
                serde_json::from_str(&connection.configuration_json)
                    .map_err(|_| "SMB configuration is invalid".to_owned())?;
            let endpoint = url::Url::parse(&connection.endpoint)
                .map_err(|_| "SMB endpoint must be a valid URL".to_owned())?;
            Ok(Box::new(
                SmbProvider::connect(SmbConfig {
                    endpoint,
                    username: stored.username,
                    password: stored.password,
                    domain: configuration.domain,
                })
                .await
                .map_err(|error| error.to_string())?,
            ))
        }
    }
}

#[tauri::command]
async fn connections_create_webdav(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateWebDavConnectionRequest,
) -> Result<ConnectionSummary, String> {
    ensure_drive_letter_unassigned(&database, request.drive_letter.as_deref(), None).await?;
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "WebDAV endpoint must be a valid URL".to_owned())?;
    let mut configuration = serde_json::json!({});
    set_drive_letter(&mut configuration, request.drive_letter.as_deref())?;
    set_mount_on_startup(&mut configuration, request.mount_on_startup)?;
    set_drive_presentation(
        &mut configuration,
        &request.drive_type,
        request.drive_icon.as_deref(),
    )?;
    create_tested_connection(
        &database,
        &credentials,
        request.name,
        ProviderKind::WebDav,
        endpoint.to_string(),
        configuration,
        serde_json::json!({ "username": request.username, "password": request.password }),
    )
    .await
}

#[tauri::command]
async fn connections_create_ftp(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateFtpConnectionRequest,
) -> Result<ConnectionSummary, String> {
    ensure_drive_letter_unassigned(&database, request.drive_letter.as_deref(), None).await?;
    if request.name.trim().is_empty()
        || request.username.trim().is_empty()
        || request.password.is_empty()
    {
        return Err("FTP connection name, username, and password are required".to_owned());
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "FTP endpoint must be a valid URL".to_owned())?;
    if !matches!(endpoint.scheme(), "ftp" | "ftps") {
        return Err("FTP endpoint must use ftp:// or ftps://".to_owned());
    }
    let mut configuration = serde_json::json!({});
    set_drive_letter(&mut configuration, request.drive_letter.as_deref())?;
    set_mount_on_startup(&mut configuration, request.mount_on_startup)?;
    set_drive_presentation(
        &mut configuration,
        &request.drive_type,
        request.drive_icon.as_deref(),
    )?;
    create_tested_connection(
        &database,
        &credentials,
        request.name,
        ProviderKind::Ftp,
        endpoint.to_string(),
        configuration,
        serde_json::json!({ "username": request.username, "password": request.password }),
    )
    .await
}

#[tauri::command]
async fn connections_create_smb(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateSmbConnectionRequest,
) -> Result<ConnectionSummary, String> {
    ensure_drive_letter_unassigned(&database, request.drive_letter.as_deref(), None).await?;
    if request.name.trim().is_empty() || request.username.trim().is_empty() {
        return Err("SMB connection name and username are required".to_owned());
    }
    let endpoint = url::Url::parse(&request.endpoint)
        .map_err(|_| "SMB endpoint must be a valid URL".to_owned())?;
    if endpoint.scheme() != "smb" {
        return Err("SMB endpoint must use smb://".to_owned());
    }
    let mut configuration = serde_json::json!({ "domain": request.domain });
    set_drive_letter(&mut configuration, request.drive_letter.as_deref())?;
    set_mount_on_startup(&mut configuration, request.mount_on_startup)?;
    set_drive_presentation(
        &mut configuration,
        &request.drive_type,
        request.drive_icon.as_deref(),
    )?;
    create_tested_connection(
        &database,
        &credentials,
        request.name,
        ProviderKind::Smb,
        endpoint.to_string(),
        configuration,
        serde_json::json!({ "username": request.username, "password": request.password }),
    )
    .await
}

#[tauri::command]
async fn connections_create_sftp(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: CreateSftpConnectionRequest,
) -> Result<ConnectionSummary, String> {
    ensure_drive_letter_unassigned(&database, request.drive_letter.as_deref(), None).await?;
    let root_path = request.root_path.trim().to_owned();
    let known_hosts = request
        .known_hosts
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(default_sftp_known_hosts);
    if request.host.trim().is_empty() || request.username.trim().is_empty() {
        return Err("SFTP host and username are required".to_owned());
    }
    if known_hosts.trim().is_empty() {
        return Err("Unable to determine the default SSH known_hosts path".to_owned());
    }
    if request.authentication == "private_key"
        && request
            .private_key_path
            .as_deref()
            .is_none_or(|path| path.trim().is_empty())
    {
        return Err("SFTP private key path is required".to_owned());
    }
    if request.authentication != "password" && request.authentication != "private_key" {
        return Err("SFTP authentication must be password or private_key".to_owned());
    }
    let authentication = request.authentication.clone();
    let mut configuration = serde_json::json!({ "host": request.host, "port": request.port, "root_path": root_path, "username": request.username, "known_hosts": known_hosts, "trust_on_first_use": request.trust_on_first_use, "authentication": authentication, "private_key_path": request.private_key_path });
    set_drive_letter(&mut configuration, request.drive_letter.as_deref())?;
    set_mount_on_startup(&mut configuration, request.mount_on_startup)?;
    set_drive_presentation(
        &mut configuration,
        &request.drive_type,
        request.drive_icon.as_deref(),
    )?;
    create_tested_connection(
        &database,
        &credentials,
        request.name,
        ProviderKind::Sftp,
        format!("sftp://{}:{}", request.host, request.port),
        configuration,
        serde_json::json!({ "username": request.username, "password": (request.authentication == "password").then_some(request.password), "private_key_path": request.private_key_path, "passphrase": request.passphrase }),
    )
    .await
}

#[tauri::command]
async fn connections_test(
    credentials: State<'_, WindowsCredentialStore>,
    request: TestConnectionRequest,
) -> Result<(), String> {
    test_connection(&credentials, request).await
}

#[tauri::command]
async fn files_list(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: ListFilesRequest,
) -> Result<FilePage, String> {
    let connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let provider = provider_for_connection(&connection, &credentials).await?;
    let path = request.path;
    let page = provider
        .list(&path, request.cursor.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    Ok(FilePage {
        entries: page
            .entries
            .into_iter()
            .map(|entry| FileSummary {
                path: entry.metadata.path,
                is_directory: entry.metadata.is_directory,
                size_bytes: entry.metadata.size_bytes,
                modified_at: entry.metadata.modified_at.map(|value| value.to_rfc3339()),
            })
            .collect(),
        next_cursor: page.next_cursor,
    })
}

#[tauri::command]
async fn files_hydrate(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: HydrateFileRequest,
) -> Result<HydrateFileResponse, String> {
    let connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let provider = provider_for_connection(&connection, &credentials).await?;
    let metadata = provider
        .stat(&request.path)
        .await
        .map_err(|error| error.to_string())?;
    let local_path = transfers
        .hydrate(
            provider.as_ref(),
            request.connection_id,
            request.path.clone(),
            metadata.size_bytes,
            request.pinned,
        )
        .await
        .map_err(|error| error.to_string())?;
    database
        .insert_activity("hydrate", Some(request.path.as_str()), "completed")
        .await
        .map_err(|error| error.to_string())?;
    Ok(HydrateFileResponse {
        path: request.path,
        local_path: local_path.display().to_string(),
    })
}

#[tauri::command]
fn sync_reconcile(request: SyncReconcileRequest) -> Result<SyncReconcileResponse, String> {
    let input = ReconciliationInput {
        base: request.base.map(Revision::new),
        local: request.local.map(Revision::new),
        remote: request.remote.map(Revision::new),
    };
    let resolution = match request.resolution.as_deref() {
        None => None,
        Some("keep_local") => Some(ConflictResolution::KeepLocal),
        Some("keep_remote") => Some(ConflictResolution::KeepRemote),
        Some("keep_both") => Some(ConflictResolution::KeepBoth),
        Some("rename_conflict") => Some(ConflictResolution::RenameConflict),
        Some(value) => return Err(format!("unknown conflict resolution: {value}")),
    };
    let decision = resolve(&input, resolution).map_err(|error| error.to_string())?;
    let conflict = matches!(decision, SyncDecision::Conflict);
    Ok(SyncReconcileResponse {
        decision: format!("{decision:?}"),
        conflict,
    })
}

#[tauri::command]
async fn sync_run(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: SyncRunRequest,
) -> Result<SyncRunResponse, String> {
    run_sync(&database, &credentials, &transfers, request).await
}

async fn run_sync(
    database: &Database,
    credentials: &WindowsCredentialStore,
    transfers: &TransferService,
    request: SyncRunRequest,
) -> Result<SyncRunResponse, String> {
    let connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let provider = provider_for_connection(&connection, credentials).await?;
    let remote = provider
        .stat(&request.path)
        .await
        .map_err(|error| error.to_string())?;
    let remote_fingerprint = remote.etag.clone().unwrap_or_else(|| {
        format!(
            "{}:{:?}",
            remote.size_bytes.unwrap_or_default(),
            remote.modified_at
        )
    });
    let input = ReconciliationInput {
        base: request.base.clone().map(Revision::new),
        local: request.local.clone().map(Revision::new),
        remote: Some(Revision::new(remote_fingerprint.clone())),
    };
    let resolution = parse_resolution(request.resolution.as_deref())?;
    let decision = resolve(&input, resolution).map_err(|error| error.to_string())?;
    let mut state = format!("{decision:?}").to_lowercase();
    let mut base_fingerprint = request.base.clone();
    let mut local_fingerprint = request.local.clone();
    let mut conflict_id = None;
    match decision {
        SyncDecision::DownloadRemote => {
            transfers
                .hydrate(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                    remote.size_bytes,
                    false,
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = Some(remote_fingerprint.clone());
            local_fingerprint = Some(remote_fingerprint.clone());
        }
        SyncDecision::UploadLocal => {
            transfers
                .upload_cached(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = Some(remote_fingerprint.clone());
            local_fingerprint = request
                .local
                .clone()
                .or_else(|| Some(remote_fingerprint.clone()));
        }
        SyncDecision::Conflict => {
            let id = uuid::Uuid::new_v4();
            database
                .insert_conflict(&ConflictRecord {
                    id,
                    connection_id: request.connection_id,
                    remote_path: request.path.to_string(),
                    local_fingerprint: request.local.clone(),
                    remote_fingerprint: Some(remote_fingerprint.clone()),
                })
                .await
                .map_err(|error| error.to_string())?;
            conflict_id = Some(id);
            state = "conflict".to_owned();
        }
        SyncDecision::Resolved(ConflictResolution::KeepRemote) => {
            transfers
                .hydrate(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                    remote.size_bytes,
                    false,
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = Some(remote_fingerprint.clone());
            local_fingerprint = Some(remote_fingerprint.clone());
        }
        SyncDecision::Resolved(ConflictResolution::KeepLocal) => {
            transfers
                .upload_cached(
                    provider.as_ref(),
                    request.connection_id,
                    request.path.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            state = "up_to_date".to_owned();
            base_fingerprint = request
                .local
                .clone()
                .or_else(|| Some(remote_fingerprint.clone()));
            local_fingerprint = base_fingerprint.clone();
        }
        SyncDecision::Resolved(ConflictResolution::KeepBoth)
        | SyncDecision::Resolved(ConflictResolution::RenameConflict) => {
            return Err(
                "this conflict resolution requires a materialized conflict filename and is not available yet"
                    .to_owned(),
            );
        }
        SyncDecision::UpToDate | SyncDecision::DeleteLocal | SyncDecision::DeleteRemote => {}
    }
    database
        .upsert_sync_entry(&SyncEntryRecord {
            connection_id: request.connection_id,
            remote_path: request.path.to_string(),
            state: state.clone(),
            base_fingerprint,
            local_fingerprint,
            remote_fingerprint: Some(remote_fingerprint),
            last_error: None,
        })
        .await
        .map_err(|error| error.to_string())?;
    database
        .insert_activity("sync", Some(request.path.as_str()), &state)
        .await
        .map_err(|error| error.to_string())?;
    Ok(SyncRunResponse {
        decision: state.clone(),
        conflict: state == "conflict",
        conflict_id,
    })
}

fn parse_resolution(value: Option<&str>) -> Result<Option<ConflictResolution>, String> {
    match value {
        None => Ok(None),
        Some("keep_local") => Ok(Some(ConflictResolution::KeepLocal)),
        Some("keep_remote") => Ok(Some(ConflictResolution::KeepRemote)),
        Some("keep_both") => Ok(Some(ConflictResolution::KeepBoth)),
        Some("rename_conflict") => Ok(Some(ConflictResolution::RenameConflict)),
        Some(value) => Err(format!("unknown conflict resolution: {value}")),
    }
}

fn is_uninstall_quit_request(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == "--quit-for-uninstall")
}

#[tauri::command]
async fn sync_conflict_resolve(
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: bifrost_api::ResolveConflictRequest,
) -> Result<(), String> {
    let resolution = parse_resolution(Some(&request.resolution))?
        .ok_or_else(|| "a conflict resolution is required".to_owned())?;
    if matches!(
        resolution,
        ConflictResolution::KeepBoth | ConflictResolution::RenameConflict
    ) {
        return Err(
            "this conflict resolution requires a materialized conflict filename and is not available yet"
                .to_owned(),
        );
    }
    let conflict = database
        .find_conflict(request.id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Conflict was not found or is already resolved".to_owned())?;
    let path = bifrost_common::RemotePath::parse(&conflict.remote_path)
        .map_err(|error| error.to_string())?;
    run_sync(
        &database,
        &credentials,
        &transfers,
        SyncRunRequest {
            connection_id: conflict.connection_id,
            path,
            base: Some("__conflict__".to_owned()),
            local: conflict.local_fingerprint.clone(),
            resolution: Some(request.resolution.clone()),
        },
    )
    .await?;
    database
        .resolve_conflict(request.id, &request.resolution)
        .await
        .map_err(|error| error.to_string())?;
    database
        .insert_activity("conflict", Some(&conflict.remote_path), &request.resolution)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn sync_conflicts_list(
    database: State<'_, Database>,
) -> Result<Vec<bifrost_api::ConflictSummary>, String> {
    database
        .list_unresolved_conflicts()
        .await
        .map(|conflicts| {
            conflicts
                .into_iter()
                .map(|conflict| bifrost_api::ConflictSummary {
                    id: conflict.id,
                    connection_id: conflict.connection_id,
                    remote_path: conflict.remote_path,
                    local_fingerprint: conflict.local_fingerprint,
                    remote_fingerprint: conflict.remote_fingerprint,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

fn start_sync_scheduler(database: Database, transfers: Arc<TransferService>) {
    tauri::async_runtime::spawn(async move {
        let credentials = WindowsCredentialStore::new();
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            let entries = match database.list_sync_entries().await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries {
                if entry.state == "conflict" {
                    continue;
                }
                let Ok(path) = bifrost_common::RemotePath::parse(&entry.remote_path) else {
                    continue;
                };
                let _ = run_sync(
                    &database,
                    &credentials,
                    &transfers,
                    SyncRunRequest {
                        connection_id: entry.connection_id,
                        path,
                        base: entry.base_fingerprint,
                        local: entry.local_fingerprint,
                        resolution: None,
                    },
                )
                .await;
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn cfapi_handler(
    provider: Arc<dyn StorageProvider>,
    transfers: Arc<TransferService>,
    connection_id: bifrost_common::ConnectionId,
) -> Arc<dyn Fn(CfapiEvent) + Send + Sync> {
    const STATUS_UNSUCCESSFUL: i32 = -1_073_741_823;

    fn remote_path_from_identity(identity: &[u8]) -> Option<bifrost_common::RemotePath> {
        if identity == b"root" {
            Some(bifrost_common::RemotePath::root())
        } else {
            bifrost_common::RemotePath::parse(&String::from_utf8_lossy(identity)).ok()
        }
    }

    Arc::new(move |event| {
        let provider = Arc::clone(&provider);
        let transfers = Arc::clone(&transfers);
        let _ = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async move {
                match &event {
                    CfapiEvent::FetchData {
                        file_identity,
                        file_offset,
                        required_length,
                        ..
                    } => {
                        let Some(path) = remote_path_from_identity(file_identity) else {
                            let _ = SyncRoot::fail_fetch_data(&event, STATUS_UNSUCCESSFUL);
                            return;
                        };
                        let end = file_offset.saturating_add(*required_length);
                        let range =
                            (*required_length > 0).then_some(*file_offset as u64..end as u64);
                        let mut stream = match provider
                            .read(bifrost_storage::ReadRequest { path, range })
                            .await
                        {
                            Ok(stream) => stream,
                            Err(error) => {
                                tracing::error!(error = %error, "CFAPI fetch-data provider read failed");
                                let _ = SyncRoot::fail_fetch_data(&event, STATUS_UNSUCCESSFUL);
                                return;
                            }
                        };
                        let mut offset = *file_offset;
                        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                            let chunk = match chunk {
                                Ok(chunk) => chunk,
                                Err(error) => {
                                    tracing::error!(error = %error, "CFAPI fetch-data stream failed");
                                    let _ = SyncRoot::fail_fetch_data(&event, STATUS_UNSUCCESSFUL);
                                    return;
                                }
                            };
                            if let Err(error) = SyncRoot::complete_fetch_data(&event, offset, &chunk) {
                                tracing::error!(error = %error, "CFAPI fetch-data completion failed");
                                return;
                            }
                            offset = offset.saturating_add(chunk.len() as i64);
                        }
                    }
                    CfapiEvent::FetchPlaceholders { file_identity, .. } => {
                        let Some(parent) = remote_path_from_identity(file_identity) else {
                            tracing::error!("CFAPI placeholder callback had an invalid file identity");
                            let _ = SyncRoot::complete_fetch_placeholders(&event, &[]);
                            return;
                        };
                        let page = match provider.list(&parent, None).await {
                            Ok(page) => page,
                            Err(error) => {
                                tracing::error!(error = %error, "CFAPI placeholder listing failed");
                                let _ = SyncRoot::complete_fetch_placeholders(&event, &[]);
                                return;
                            }
                        };
                        let entries = page
                            .entries
                            .into_iter()
                            .map(|entry| {
                                let path = entry.metadata.path.as_str();
                                let relative = if parent.as_str().is_empty() {
                                    path.to_owned()
                                } else {
                                    path.strip_prefix(&format!("{}/", parent.as_str()))
                                        .unwrap_or(path)
                                        .to_owned()
                                };
                                PlaceholderMetadata {
                                    relative_name: relative,
                                    identity: entry.metadata.path.as_str().as_bytes().to_vec(),
                                    remote: entry.metadata,
                                }
                            })
                            .collect::<Vec<_>>();
                        if let Err(error) = SyncRoot::complete_fetch_placeholders(&event, &entries) {
                            tracing::error!(error = %error, "CFAPI placeholder completion failed");
                        }
                    }
                    CfapiEvent::NotifyFileClose { file_identity } => {
                        let Ok(path) = bifrost_common::RemotePath::parse(&String::from_utf8_lossy(
                            file_identity,
                        )) else {
                            return;
                        };
                        let _ = transfers
                            .upload_cached(provider.as_ref(), connection_id, path)
                            .await;
                    }
                    CfapiEvent::NotifyDelete { file_identity } => {
                        let Ok(path) = bifrost_common::RemotePath::parse(&String::from_utf8_lossy(
                            file_identity,
                        )) else {
                            return;
                        };
                        let _ = provider.delete(&path).await;
                    }
                    CfapiEvent::NotifyRename {
                        file_identity,
                        target_path,
                    } => {
                        let Ok(path) = bifrost_common::RemotePath::parse(&String::from_utf8_lossy(
                            file_identity,
                        )) else {
                            return;
                        };
                        let Ok(target) = bifrost_common::RemotePath::parse(target_path) else {
                            return;
                        };
                        let _ = provider.rename(&path, &target).await;
                    }
                }
            });
        })
        .join();
    })
}

#[tauri::command]
async fn sync_root_register(
    registry: State<'_, SyncRootRegistry>,
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    transfers: State<'_, Arc<TransferService>>,
    request: bifrost_api::SyncRootRegisterRequest,
) -> Result<bifrost_api::SyncRootRegisterResponse, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (registry, database, credentials, transfers, request);
        Err("Windows CFAPI is available only on Windows".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        let connection = database
            .find_connection(request.connection_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Connection was not found".to_owned())?;
        let provider = provider_for_connection(&connection, &credentials).await?;
        let path = request
            .path
            .map(PathBuf::from)
            .unwrap_or_else(|| default_sync_root_path(&connection));
        if !path.is_absolute() {
            return Err("Sync root path must be absolute".to_owned());
        }
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        let mut provider_id = [0; 16];
        provider_id.copy_from_slice(connection.id.as_uuid().as_bytes());
        let root = SyncRoot::register(SyncRootConfig {
            path: path.clone(),
            provider_name: "Bifrost Drive".to_owned(),
            provider_version: env!("CARGO_PKG_VERSION").to_owned(),
            provider_id,
            sync_root_identity: path.to_string_lossy().as_bytes().to_vec(),
            root_file_identity: b"root".to_vec(),
        })
        .map_err(|error| error.to_string())?
        .connect_with_handler(cfapi_handler(
            Arc::from(provider),
            Arc::clone(&transfers),
            request.connection_id,
        ))
        .map_err(|error| error.to_string())?;
        let previous = registry.insert(connection.id.to_string(), root);
        drop(previous);
        Ok(bifrost_api::SyncRootRegisterResponse {
            path: path.to_string_lossy().to_string(),
        })
    }
}

#[tauri::command]
fn sync_root_unregister(
    registry: State<'_, SyncRootRegistry>,
    request: ConnectionIdRequest,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (registry, request);
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        drop(registry.take(&request.id.to_string()));
        Ok(())
    }
}

#[tauri::command]
fn drive_letters_available() -> Result<Vec<String>, String> {
    #[cfg(not(target_os = "windows"))]
    {
        Err("Windows drive mounting is available only on Windows".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        let used = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
        Ok(('D'..='Z')
            .rev()
            .filter(|letter| used & (1 << (*letter as u32 - 'A' as u32)) == 0)
            .map(|letter| format!("{letter}:"))
            .collect())
    }
}

#[tauri::command]
async fn drive_mount_register(
    app: tauri::AppHandle,
    registry: State<'_, DriveMountRegistry>,
    database: State<'_, Database>,
    credentials: State<'_, WindowsCredentialStore>,
    request: DriveMountRegisterRequest,
) -> Result<DriveMountRegisterResponse, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, registry, database, credentials, request);
        Err("Windows drive mounting is available only on Windows".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        let connection = database
            .find_connection(request.connection_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Connection was not found".to_owned())?;
        let Some(drive_letter) = configured_drive_letter(&connection.configuration_json)? else {
            return Err("No drive letter is assigned to this connection".to_owned());
        };
        let key = connection.id.to_string();
        if registry.contains(&key) {
            return Ok(DriveMountRegisterResponse {
                drive_letter: format!("{drive_letter}:"),
            });
        }
        let provider = provider_for_connection(&connection, &credentials).await?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let handle = bifrost_windows_winfsp::mount(MountConfig {
            drive_letter,
            volume_label: connection.name.clone(),
            network_drive: configured_drive_type(&connection.configuration_json)?,
            icon_source: configured_drive_icon(
                &connection.configuration_json,
                &data_dir,
                connection.id,
            )?,
            provider: Arc::from(provider),
        })
        .map_err(|error| error.to_string())?;
        let previous = registry.insert(key, handle);
        drop(previous);
        record_connection_activity(&database, "drive_mounted", &connection).await;
        Ok(DriveMountRegisterResponse {
            drive_letter: format!("{drive_letter}:"),
        })
    }
}

#[tauri::command]
async fn drive_mount_unregister(
    registry: State<'_, DriveMountRegistry>,
    database: State<'_, Database>,
    request: ConnectionIdRequest,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (registry, database, request);
        Err("Windows drive mounting is available only on Windows".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        let removed = registry.take(&request.id.to_string());
        let was_mounted = removed.is_some();
        drop(removed);
        if was_mounted {
            if let Some(connection) = database
                .find_connection(request.id)
                .await
                .map_err(|error| error.to_string())?
            {
                record_connection_activity(&database, "drive_unmounted", &connection).await;
            }
        }
        Ok(())
    }
}

#[tauri::command]
async fn drive_mount_startup_set(
    database: State<'_, Database>,
    request: DriveMountStartupRequest,
) -> Result<(), String> {
    let mut connection = database
        .find_connection(request.connection_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Connection was not found".to_owned())?;
    let mut configuration: serde_json::Value = serde_json::from_str(&connection.configuration_json)
        .map_err(|_| "Connection configuration is invalid".to_owned())?;
    set_mount_on_startup(&mut configuration, request.enabled)?;
    connection.configuration_json = configuration.to_string();
    database
        .update_connection(&connection)
        .await
        .map_err(|error| error.to_string())?;
    record_connection_activity(
        &database,
        if request.enabled {
            "startup_mount_enabled"
        } else {
            "startup_mount_disabled"
        },
        &connection,
    )
    .await;
    Ok(())
}

#[tauri::command]
async fn connection_location_open(
    registry: State<'_, DriveMountRegistry>,
    database: State<'_, Database>,
    request: ConnectionIdRequest,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (registry, database, request);
        Err("Opening connection locations is available only on Windows".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        let connection = database
            .find_connection(request.id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Connection was not found".to_owned())?;
        let path = if let Some(letter) = configured_drive_letter(&connection.configuration_json)? {
            if !registry.contains(&connection.id.to_string()) {
                return Err("The drive is not mounted".to_owned());
            }
            PathBuf::from(format!(r"{letter}:\"))
        } else {
            default_sync_root_path(&connection)
        };
        std::process::Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map_err(|error| error.to_string())?;
        record_connection_activity(&database, "explorer_opened", &connection).await;
        Ok(())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .compact()
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, arguments, _| {
            if is_uninstall_quit_request(&arguments) {
                app.exit(0);
                return;
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            if is_uninstall_quit_request(&std::env::args().collect::<Vec<_>>()) {
                app.handle().exit(0);
                return Ok(());
            }
            #[cfg(target_os = "windows")]
            if let Err(error) = cleanup_bifrost_mount_points() {
                tracing::warn!(%error, "could not clear stale Explorer mount metadata");
            }
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let database_path = data_dir.join("bifrost-drive.db");
            let database = tauri::async_runtime::block_on(Database::connect_file(&database_path))?;
            tauri::async_runtime::block_on(database.migrate())?;
            let cache = CacheManager::new(data_dir.join("cache"), 1024 * 1024 * 1024)?;
            let transfer_store = Arc::new(SqliteTransferStore {
                database: database.clone(),
            });
            let transfers = TransferService::with_store(cache, 4, 5, Some(transfer_store));
            tauri::async_runtime::block_on(transfers.recover())?;
            let transfers = Arc::new(transfers);
            start_sync_scheduler(database.clone(), Arc::clone(&transfers));
            app.manage(database);
            app.manage(transfers);
            let open = MenuItemBuilder::with_id("open", "Open Bifrost Drive").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;
            let mut tray = TrayIconBuilder::new().menu(&menu);
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            tray.on_menu_event(|app, event| match event.id().as_ref() {
                "open" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            })
            .build(app)?;
            let preferences = load_preferences(&data_dir.join("preferences.json"))
                .map_err(std::io::Error::other)?;
            if !preferences.start_minimized {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .manage(Mutex::new(Application::new()))
        .manage(WindowsCredentialStore::new())
        .manage(SyncRootRegistry::new())
        .manage(DriveMountRegistry::new())
        .invoke_handler(tauri::generate_handler![
            app_status,
            app_start_minimized_get,
            app_start_minimized_set,
            connections_list,
            connections_details,
            connections_update,
            activity_list,
            connections_create,
            connections_create_s3,
            connections_create_ftp,
            connections_create_smb,
            connections_create_webdav,
            connections_create_sftp,
            connections_remove,
            connections_test,
            files_list,
            files_hydrate,
            credentials_store_s3,
            sync_reconcile,
            sync_run,
            sync_conflicts_list,
            sync_conflict_resolve,
            sync_root_register,
            sync_root_unregister,
            drive_letters_available,
            drive_stock_icons,
            drive_icon_preview,
            drive_mount_register,
            drive_mount_unregister,
            drive_mount_startup_set,
            connection_location_open
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bifrost Drive");
}

#[cfg(target_os = "windows")]
pub fn cleanup_windows_integrations() -> Result<(), String> {
    let root = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "USERPROFILE is unavailable".to_owned())?
        .join("Bifrost Drive");
    let mut failures = Vec::new();
    if root.is_dir() {
        for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                if let Err(error) = bifrost_windows_cfapi::unregister(&path) {
                    failures.push(format!("{}: {error}", path.display()));
                }
            }
        }
    }
    if let Err(error) = cleanup_bifrost_mount_points() {
        failures.push(error);
    }
    if let Err(error) = cleanup_configured_drive_icons() {
        failures.push(error);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(target_os = "windows")]
fn cleanup_configured_drive_icons() -> Result<(), String> {
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
            System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER},
        },
    };
    let Some(app_data) = std::env::var_os("APPDATA") else {
        return Ok(());
    };
    let database_path = PathBuf::from(app_data)
        .join("com.bifrost.drive")
        .join("bifrost-drive.db");
    if !database_path.is_file() {
        return Ok(());
    }
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    let connections = runtime.block_on(async {
        let database = Database::connect_file(&database_path).await?;
        database.list_connections().await
    });
    for connection in connections.map_err(|error| error.to_string())? {
        let Some(letter) = configured_drive_letter(&connection.configuration_json)? else {
            continue;
        };
        let key = windows_wide_null(&format!(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\DriveIcons\{letter}"
        ));
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            return Err(format!(
                "could not remove Explorer drive override for {letter}: {}",
                status.0
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn cleanup_bifrost_mount_points() -> Result<(), String> {
    cleanup_bifrost_mount_points_matching(is_bifrost_mount_point)
}

#[cfg(target_os = "windows")]
fn cleanup_bifrost_mount_points_for_share(share: &str) -> Result<(), String> {
    let share = share
        .chars()
        .map(|character| match character {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();
    let share = share.trim().trim_matches('.').to_ascii_lowercase();
    cleanup_bifrost_mount_points_matching(|entry| {
        is_bifrost_mount_point(entry) && entry.to_ascii_lowercase().ends_with(&format!("#{share}"))
    })
}

#[cfg(target_os = "windows")]
fn cleanup_bifrost_mount_points_matching(
    should_remove: impl Fn(&str) -> bool,
) -> Result<(), String> {
    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS},
            System::Registry::{
                RegCloseKey, RegDeleteTreeW, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
                KEY_ENUMERATE_SUB_KEYS, KEY_WRITE,
            },
            UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_FLUSH, SHCNF_IDLIST},
        },
    };
    let key_path =
        windows_wide_null(r"Software\Microsoft\Windows\CurrentVersion\Explorer\MountPoints2");
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_ENUMERATE_SUB_KEYS | KEY_WRITE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status != ERROR_SUCCESS {
        return Err(format!(
            "could not open Explorer mount metadata: {}",
            status.0
        ));
    }
    let mut index = 0;
    loop {
        let mut name = [0u16; 512];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            unsafe {
                let _ = RegCloseKey(key);
            }
            return Err(format!(
                "could not enumerate Explorer mount metadata: {}",
                status.0
            ));
        }
        let entry = String::from_utf16_lossy(&name[..length as usize]);
        if should_remove(&entry) {
            let entry = windows_wide_null(&entry);
            let _ = unsafe { RegDeleteTreeW(key, PCWSTR(entry.as_ptr())) };
        } else {
            index += 1;
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST | SHCNF_FLUSH, None, None);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn is_bifrost_mount_point(entry: &str) -> bool {
    entry.to_ascii_lowercase().starts_with("##bifrost")
}

#[cfg(all(test, target_os = "windows"))]
mod registry_tests {
    use super::{
        is_bifrost_mount_point, set_drive_presentation, shell32_icons, DriveMountRegistry,
    };

    #[test]
    fn drive_registry_recovers_from_poisoning() {
        let registry = DriveMountRegistry::new();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = registry.0.lock().unwrap();
            panic!("poison registry for regression coverage");
        }));

        assert!(registry.0.is_poisoned());
        assert!(!registry.contains("missing"));
        assert!(!registry.0.is_poisoned());
    }

    #[test]
    fn validates_drive_presentation_settings() {
        let mut configuration = serde_json::json!({});
        set_drive_presentation(&mut configuration, "local", Some("bifrost")).unwrap();
        assert_eq!(configuration["drive_type"], "local");
        assert_eq!(configuration["drive_icon"], "bifrost");
        assert!(set_drive_presentation(&mut configuration, "portable", None).is_err());
    }

    #[test]
    fn matches_only_bifrost_mount_metadata() {
        assert!(is_bifrost_mount_point("##bifrost-123#Yggdrasil"));
        assert!(!is_bifrost_mount_point("##server#share"));
    }

    #[test]
    fn extracts_the_full_shell32_icon_catalog() {
        let icons = shell32_icons().unwrap();
        assert!(icons.len() > 200);
        assert!(icons
            .iter()
            .all(|icon| icon.preview.starts_with("data:image/png;base64,")));
    }
}

#[cfg(test)]
mod preferences_tests {
    use super::{is_uninstall_quit_request, load_preferences, save_preferences, AppPreferences};

    #[test]
    fn start_minimized_preference_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");

        assert!(!load_preferences(&path).unwrap().start_minimized);
        save_preferences(
            &path,
            &AppPreferences {
                start_minimized: true,
            },
        )
        .unwrap();
        assert!(load_preferences(&path).unwrap().start_minimized);
        save_preferences(&path, &AppPreferences::default()).unwrap();
        assert!(!load_preferences(&path).unwrap().start_minimized);
    }

    #[test]
    fn recognizes_only_the_uninstall_quit_switch() {
        assert!(is_uninstall_quit_request(&[
            "bifrost-drive.exe".to_owned(),
            "--quit-for-uninstall".to_owned(),
        ]));
        assert!(!is_uninstall_quit_request(&[
            "bifrost-drive.exe".to_owned(),
            "--cleanup-windows-integrations".to_owned(),
        ]));
    }
}
