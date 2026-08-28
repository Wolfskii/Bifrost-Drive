import { invoke } from "@tauri-apps/api/core";
import {
    disable as disableAutostart,
    enable as enableAutostart,
    isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

export interface UpdateInfo {
    version: string;
    body: string;
}

export type ProviderKind =
    "S3" | "Sftp" | "WebDav" | "Nextcloud" | "GoogleDrive" | "Ftp" | "Smb";

export interface ConnectionSummary {
    id: string;
    name: string;
    kind: ProviderKind;
    state: string;
    endpoint: string;
}

export interface CredentialStoreStatus {
    available: boolean;
    platform: string;
    provider: string;
    message: string | null;
    desktop_environment: string | null;
    linux_distribution: string | null;
}

export interface ConnectionDetails {
    summary: ConnectionSummary;
    configuration: Record<string, unknown>;
    username: string | null;
    drive_icon_preview: string | null;
}

export interface FileSummary {
    path: string;
    is_directory: boolean;
    size_bytes: number | null;
    modified_at: string | null;
}

export interface S3ConnectionForm {
    name: string;
    endpoint: string;
    region: string;
    bucket: string;
    pathStyle: boolean;
    accessKeyId: string;
    secretAccessKey: string;
    driveLetter: string;
    mountOnStartup: boolean;
    mountRoot: string;
    driveType: "network" | "local";
    driveIcon: string;
}

export interface GoogleDriveConnectionForm {
    name: string;
    accessToken: string;
    refreshToken: string | null;
    expiresAt: number | null;
    sharedDriveId: string;
    driveLetter: string;
    mountOnStartup: boolean;
    mountRoot: string;
    driveType: "network" | "local";
    driveIcon: string;
}

export interface GoogleDriveAuthorization {
    access_token: string;
    refresh_token: string;
    expires_at: number;
}

export interface WebDavConnectionForm {
    name: string;
    endpoint: string;
    username: string;
    password: string;
    driveLetter: string;
    mountOnStartup: boolean;
    mountRoot: string;
    driveType: "network" | "local";
    driveIcon: string;
}

export interface FtpConnectionForm {
    name: string;
    endpoint: string;
    username: string;
    password: string;
    driveLetter: string;
    mountOnStartup: boolean;
    mountRoot: string;
    driveType: "network" | "local";
    driveIcon: string;
}

export interface SmbConnectionForm {
    name: string;
    endpoint: string;
    username: string;
    password: string;
    domain: string;
    driveLetter: string;
    mountOnStartup: boolean;
    mountRoot: string;
    driveType: "network" | "local";
    driveIcon: string;
}

export interface SftpConnectionForm {
    name: string;
    host: string;
    port: number;
    rootPath: string;
    username: string;
    password: string;
    authentication: "password" | "private_key";
    trustOnFirstUse: boolean;
    privateKeyPath: string;
    passphrase: string;
    driveLetter: string;
    mountOnStartup: boolean;
    mountRoot: string;
    driveType: "network" | "local";
    driveIcon: string;
}

export interface HydrateFileResponse {
    path: string;
    local_path: string;
}

export interface SyncRunResponse {
    decision: string;
    conflict: boolean;
    conflict_id: string | null;
}

export interface ConflictSummary {
    id: string;
    connection_id: string;
    remote_path: string;
    local_fingerprint: string | null;
    remote_fingerprint: string | null;
}

export interface ActivitySummary {
    id: string;
    kind: string;
    remote_path: string | null;
    status: string;
    created_at: string;
}

export interface SyncRootRegisterResponse {
    path: string;
}

export interface DriveMountRegisterResponse {
    location: string;
    drive_letter: string | null;
}

export type FilesystemIntegration = "windows" | "linux" | "macos" | "none";

export interface StockDriveIcon {
    value: string;
    label: string;
    preview: string;
}

export async function getDriveIconPreview(path: string): Promise<string> {
    if (!tauriAvailable()) {
        return "";
    }
    return invoke<string>("drive_icon_preview", { request: { path } });
}

function tauriAvailable(): boolean {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function listConnections(): Promise<ConnectionSummary[]> {
    if (!tauriAvailable()) {
        return [];
    }
    return invoke<ConnectionSummary[]>("connections_list");
}

export async function supportsSyncRoots(): Promise<boolean> {
    if (!tauriAvailable()) {
        return false;
    }
    return invoke<boolean>("sync_root_supported");
}

export async function getFilesystemIntegration(): Promise<FilesystemIntegration> {
    if (!tauriAvailable()) {
        return "none";
    }
    return invoke<FilesystemIntegration>("filesystem_integration_kind");
}

export async function getFilesystemDefaultMountRoot(): Promise<string> {
    if (!tauriAvailable()) {
        return "";
    }
    return (await invoke<string | null>("filesystem_default_mount_root")) ?? "";
}

export async function getConnectionDetails(
    connectionId: string,
): Promise<ConnectionDetails> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionDetails>("connections_details", {
        request: { id: connectionId },
    });
}

export async function removeConnection(connectionId: string): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("connections_remove", {
        request: { id: connectionId },
    });
}

export async function updateConnection(request: {
    id: string;
    name: string;
    endpoint: string;
    configuration: Record<string, unknown>;
    credentials: Record<string, unknown>;
}): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_update", { request });
}

export async function getAutostartEnabled(): Promise<boolean> {
    if (!tauriAvailable()) {
        return false;
    }
    return isAutostartEnabled();
}

export async function setAutostartEnabled(enabled: boolean): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    if (enabled) {
        await enableAutostart();
    } else {
        await disableAutostart();
    }
}

export async function getStartMinimized(): Promise<boolean> {
    if (!tauriAvailable()) {
        return false;
    }
    return invoke<boolean>("app_start_minimized_get");
}

export async function getCredentialStoreStatus(): Promise<CredentialStoreStatus> {
    if (!tauriAvailable()) {
        return {
            available: true,
            platform: "browser",
            provider: "Browser preview",
            message: null,
            desktop_environment: null,
            linux_distribution: null,
        };
    }
    return invoke<CredentialStoreStatus>("credential_store_check");
}

export async function restartApp(): Promise<void> {
    if (!tauriAvailable()) return;
    await relaunch();
}

export async function setStartMinimized(enabled: boolean): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("app_start_minimized_set", { enabled });
}

export async function getUpdatePopupsEnabled(): Promise<boolean> {
    if (!tauriAvailable()) {
        return true;
    }
    return invoke<boolean>("app_update_popups_get");
}

export async function setUpdatePopupsEnabled(enabled: boolean): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("app_update_popups_set", { enabled });
}

export async function checkForUpdate(): Promise<UpdateInfo | null> {
    if (!tauriAvailable()) {
        return null;
    }
    const update = await check();
    return update ? { version: update.version, body: update.body ?? "" } : null;
}

export async function installUpdate(): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    const update = await check();
    if (!update) return;
    await update.downloadAndInstall();
    await relaunch();
}

export async function createS3Connection(
    form: S3ConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }

    return invoke<ConnectionSummary>("connections_create_s3", {
        request: {
            name: form.name,
            endpoint: form.endpoint,
            region: form.region,
            bucket: form.bucket,
            path_style: form.pathStyle,
            access_key_id: form.accessKeyId,
            secret_access_key: form.secretAccessKey,
            drive_letter: form.driveLetter || null,
            mount_on_startup: form.mountOnStartup,
            mount_root: form.mountRoot || null,
            drive_type: form.driveType,
            drive_icon: form.driveIcon || null,
        },
    });
}

export async function createGoogleDriveConnection(
    form: GoogleDriveConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_google_drive", {
        request: {
            name: form.name,
            access_token: form.accessToken,
            refresh_token: form.refreshToken,
            expires_at: form.expiresAt,
            shared_drive_id: form.sharedDriveId || null,
            drive_letter: form.driveLetter || null,
            mount_on_startup: form.mountOnStartup,
            mount_root: form.mountRoot || null,
            drive_type: form.driveType,
            drive_icon: form.driveIcon || null,
        },
    });
}

export async function authorizeGoogleDrive(): Promise<GoogleDriveAuthorization> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<GoogleDriveAuthorization>(
        "connections_google_drive_authorize",
    );
}

export async function createWebDavConnection(
    form: WebDavConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_webdav", {
        request: {
            ...form,
            drive_letter: form.driveLetter || null,
            mount_on_startup: form.mountOnStartup,
            mount_root: form.mountRoot || null,
            drive_type: form.driveType,
            drive_icon: form.driveIcon || null,
        },
    });
}

export async function createFtpConnection(
    form: FtpConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_ftp", {
        request: {
            ...form,
            drive_letter: form.driveLetter || null,
            mount_on_startup: form.mountOnStartup,
            mount_root: form.mountRoot || null,
            drive_type: form.driveType,
            drive_icon: form.driveIcon || null,
        },
    });
}

export async function createSmbConnection(
    form: SmbConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_smb", {
        request: {
            ...form,
            drive_letter: form.driveLetter || null,
            mount_on_startup: form.mountOnStartup,
            mount_root: form.mountRoot || null,
            drive_type: form.driveType,
            drive_icon: form.driveIcon || null,
        },
    });
}

export async function createSftpConnection(
    form: SftpConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_sftp", {
        request: {
            ...form,
            root_path: form.rootPath,
            trust_on_first_use: form.trustOnFirstUse,
            drive_letter: form.driveLetter || null,
            mount_on_startup: form.mountOnStartup,
            mount_root: form.mountRoot || null,
            drive_type: form.driveType,
            drive_icon: form.driveIcon || null,
        },
    });
}

export async function getAvailableDriveLetters(): Promise<string[]> {
    if (!tauriAvailable()) {
        return [];
    }
    return invoke<string[]>("drive_letters_available");
}

export async function getStockDriveIcons(): Promise<StockDriveIcon[]> {
    if (!tauriAvailable()) {
        return [];
    }
    return invoke<StockDriveIcon[]>("drive_stock_icons");
}

export async function registerDriveMount(
    connectionId: string,
): Promise<DriveMountRegisterResponse> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<DriveMountRegisterResponse>("drive_mount_register", {
        request: { connection_id: connectionId },
    });
}

export async function unregisterDriveMount(
    connectionId: string,
): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("drive_mount_unregister", {
        request: { id: connectionId },
    });
}

export async function setDriveMountStartup(
    connectionId: string,
    enabled: boolean,
): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("drive_mount_startup_set", {
        request: { connection_id: connectionId, enabled },
    });
}

export async function openConnectionLocation(
    connectionId: string,
): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("connection_location_open", {
        request: { id: connectionId },
    });
}

export async function listFiles(connectionId: string): Promise<FileSummary[]> {
    if (!tauriAvailable()) {
        return [];
    }
    const page = await invoke<{
        entries: FileSummary[];
        next_cursor: string | null;
    }>("files_list", {
        request: {
            connection_id: connectionId,
            path: "",
            cursor: null,
        },
    });
    return page.entries;
}

export async function hydrateFile(
    connectionId: string,
    path: string,
    pinned = false,
): Promise<HydrateFileResponse> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<HydrateFileResponse>("files_hydrate", {
        request: {
            connection_id: connectionId,
            path,
            pinned,
        },
    });
}

export async function runSync(
    connectionId: string,
    path: string,
    base: string | null = null,
    local: string | null = null,
    resolution: string | null = null,
): Promise<SyncRunResponse> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<SyncRunResponse>("sync_run", {
        request: {
            connection_id: connectionId,
            path,
            base,
            local,
            resolution,
        },
    });
}

export async function listConflicts(): Promise<ConflictSummary[]> {
    if (!tauriAvailable()) {
        return [];
    }
    return invoke<ConflictSummary[]>("sync_conflicts_list");
}

export async function listActivity(): Promise<ActivitySummary[]> {
    if (!tauriAvailable()) {
        return [];
    }
    return invoke<ActivitySummary[]>("activity_list");
}

export async function resolveConflict(
    id: string,
    resolution: "keep_local" | "keep_remote",
): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("sync_conflict_resolve", {
        request: { id, resolution },
    });
}

export async function registerSyncRoot(
    connectionId: string,
): Promise<SyncRootRegisterResponse> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<SyncRootRegisterResponse>("sync_root_register", {
        request: { connection_id: connectionId },
    });
}

export async function unregisterSyncRoot(connectionId: string): Promise<void> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    await invoke("sync_root_unregister", {
        request: { id: connectionId },
    });
}
