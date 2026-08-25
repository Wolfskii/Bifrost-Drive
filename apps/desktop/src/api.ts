import { invoke } from "@tauri-apps/api/core";

export type ProviderKind = "S3" | "Sftp" | "WebDav" | "Nextcloud";

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

export interface SftpConnectionForm {
    name: string;
    host: string;
    port: number;
    username: string;
    password: string;
    knownHosts: string;
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

export async function createSftpConnection(
    form: SftpConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error("The desktop service is not available in this window");
    }
    return invoke<ConnectionSummary>("connections_create_sftp", {
        request: {
            ...form,
            known_hosts: form.knownHosts,
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
