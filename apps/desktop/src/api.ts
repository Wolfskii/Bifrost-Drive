import { invoke } from '@tauri-apps/api/core';

export type ProviderKind = 'S3' | 'Sftp' | 'WebDav' | 'Nextcloud';

export interface ConnectionSummary {
    id: string;
    name: string;
    kind: ProviderKind;
    state: string;
    endpoint: string;
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

function tauriAvailable(): boolean {
    return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export async function listConnections(): Promise<ConnectionSummary[]> {
    if (!tauriAvailable()) {
        return [];
    }
    return invoke<ConnectionSummary[]>('connections_list');
}

export async function createS3Connection(
    form: S3ConnectionForm,
): Promise<ConnectionSummary> {
    if (!tauriAvailable()) {
        throw new Error('The desktop service is not available in this window');
    }

    const credential = await invoke<{ id: string; kind: string; label: string }>(
        'credentials_store_s3',
        {
            request: {
                label: form.name,
                access_key_id: form.accessKeyId,
                secret_access_key: form.secretAccessKey,
            },
        },
    );

    return invoke<ConnectionSummary>('connections_create', {
        request: {
            name: form.name,
            kind: 'S3',
            endpoint: form.endpoint,
            credential_ref: JSON.stringify(credential),
            configuration: {
                region: form.region,
                bucket: form.bucket,
                path_style: form.pathStyle,
            },
        },
    });
}
