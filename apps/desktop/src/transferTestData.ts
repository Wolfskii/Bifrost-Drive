import type { TransferSnapshot, TransferView } from "./api";

export function snapshot(
    overrides: Partial<TransferSnapshot> = {},
): TransferSnapshot {
    return {
        state: "running",
        mode: "copy",
        total_files: 2000,
        total_bytes: 1000,
        done_files: 1204,
        done_bytes: 710,
        skipped_files: 0,
        failed_files: 0,
        failures: [],
        current: [],
        bytes_per_second: 0,
        seconds_left: null,
        conflict: null,
        ...overrides,
    };
}

export function view(
    overrides: Partial<TransferSnapshot> = {},
    extra: Partial<TransferView> = {},
): TransferView {
    return {
        id: 1,
        connection: "Yggdrasil SFTP",
        direction: "upload",
        items: 3,
        first_item: "clips",
        destination: "Y:\\Filmer",
        started_at_ms: 0,
        snapshot: snapshot(overrides),
        ...extra,
    };
}
