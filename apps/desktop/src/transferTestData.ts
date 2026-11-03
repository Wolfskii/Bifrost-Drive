import type {
    TransferHistoryEntry,
    TransferSnapshot,
    TransferView,
} from "./api";

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
        files_per_second: 0,
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

export function historyEntry(
    overrides: Partial<TransferHistoryEntry> = {},
): TransferHistoryEntry {
    return {
        id: 1,
        started_at: "2026-11-02T20:00:00.000Z",
        finished_at: "2026-11-02T20:03:04.000Z",
        mode: "copy",
        direction: "upload",
        connection: "Yggdrasil SFTP",
        destination: "Y:\\Filmer",
        items: ["C:\\Videos\\a.mkv", "C:\\Videos\\b.mkv", "C:\\Videos\\c.mkv"],
        item_count: 3,
        state: "completed",
        total_files: 3,
        done_files: 3,
        skipped_files: 0,
        failed_files: 0,
        total_bytes: 3 * 1024 * 1024,
        done_bytes: 3 * 1024 * 1024,
        failures: [],
        live: null,
        ...overrides,
    };
}
