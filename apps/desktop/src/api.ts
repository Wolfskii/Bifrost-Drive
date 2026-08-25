import { invoke } from "@tauri-apps/api/core";
import {
    disable as disableAutostart,
    enable as enableAutostart,
    isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

export type ProviderKind =
    "S3" | "Sftp" | "WebDav" | "Nextcloud" | "Ftp" | "Smb";

export interface ConnectionSummary {
    id: string;
    name: string;
    kind: ProviderKind;
    state: string;
    endpoint: string;
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
}

export interface WebDavConnectionForm {
    name: string;
    endpoint: string;
    username: string;
    password: string;
}

export interface FtpConnectionForm {
    name: string;
    endpoint: string;
    username: string;
    password: string;
}

export interface SmbConnectionForm {
    name: string;
    endpoint: string;
    username: string;
    password: string;
    domain: string;
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

export async function checkForUpdate(): Promise<string | null> {
    if (!tauriAvailable()) {
        return null;
    }
    const update = await check();
    return update?.version ?? null;
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
        },
    });
}

export async function createWebDavConnection(
    form: WebDavConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_webdav", {
        request: form,
    });
}

export async function createFtpConnection(
    form: FtpConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_ftp", {
        request: form,
    });
}

export async function createSmbConnection(
    form: SmbConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_smb", {
        request: form,
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
        },
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
