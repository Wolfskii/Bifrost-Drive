import type {
    TransferFailure,
    TransferHistoryEntry,
    TransferSnapshot,
    TransferView,
} from "./api";

export const SEGMENT_COUNT = 28;

const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number): string {
    let value = Math.max(0, bytes);
    let unit = 0;
    while (value >= 1024 && unit < UNITS.length - 1) {
        value /= 1024;
        unit += 1;
    }
    const digits = unit === 0 || value >= 100 ? 0 : 1;
    return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

export function formatSpeed(bytesPerSecond: number): string {
    return `${formatBytes(bytesPerSecond)}/s`;
}

/** Speed shown on the card: data per second, or files per second when nothing is copied. */
export function formatRate(snapshot: TransferSnapshot): string {
    return snapshot.mode === "delete"
        ? `${formatCount(snapshot.files_per_second)} files/s`
        : formatSpeed(snapshot.bytes_per_second);
}

export function formatCount(count: number): string {
    return count.toLocaleString("en-US");
}

function plural(count: number, noun: string): string {
    return `${formatCount(count)} ${noun}${count === 1 ? "" : "s"}`;
}

/** "about 3 min", "less than a minute", "about 1 h 5 min". */
export function formatDuration(seconds: number): string {
    if (seconds < 60) return "less than a minute";
    const minutes = Math.ceil(seconds / 60);
    if (minutes < 60) return `about ${minutes} min`;
    const hours = Math.floor(minutes / 60);
    const rest = minutes % 60;
    return rest === 0 ? `about ${hours} h` : `about ${hours} h ${rest} min`;
}

export function isFinished(snapshot: TransferSnapshot): boolean {
    return [
        "completed",
        "completed_with_errors",
        "cancelled",
        "failed",
    ].includes(snapshot.state);
}

/** Progress from 0 to 100, by bytes when the batch has any and by file count otherwise. */
export function percentDone(snapshot: TransferSnapshot): number {
    if (snapshot.state === "completed") return 100;
    const ratio =
        snapshot.total_bytes > 0
            ? snapshot.done_bytes / snapshot.total_bytes
            : snapshot.total_files > 0
              ? (snapshot.done_files + snapshot.skipped_files) /
                snapshot.total_files
              : 0;
    return Math.min(100, Math.max(0, Math.floor(ratio * 100)));
}

/** How full each bar of the segmented progress bar is, from 0 to 1. */
export function segmentFills(
    percent: number,
    count: number = SEGMENT_COUNT,
): number[] {
    const filled = (percent / 100) * count;
    return Array.from({ length: count }, (_, index) =>
        Math.min(1, Math.max(0, filled - index)),
    );
}

export function cardTitle(view: TransferView): string {
    const { snapshot } = view;
    const moving = snapshot.mode === "move";
    const deleting = snapshot.mode === "delete";
    const subject =
        snapshot.total_files === 1 && view.items === 1
            ? view.first_item
            : plural(snapshot.total_files, "file");
    if (deleting) {
        switch (snapshot.state) {
            case "planning":
                return "Getting ready…";
            case "completed":
                return `Deleted ${subject}`;
            case "completed_with_errors":
                return `Deleted ${formatCount(snapshot.done_files)} of ${plural(snapshot.total_files, "file")}`;
            case "cancelled":
                return "Delete cancelled";
            case "failed":
                return "Couldn't start deleting";
            default:
                return `Deleting ${subject}`;
        }
    }
    switch (snapshot.state) {
        case "planning":
            return "Getting ready…";
        case "completed":
            return `${moving ? "Moved" : "Copied"} ${subject}`;
        case "completed_with_errors":
            return `${moving ? "Moved" : "Copied"} ${formatCount(snapshot.done_files)} of ${plural(snapshot.total_files, "file")}`;
        case "cancelled":
            return "Transfer cancelled";
        case "failed":
            return "Couldn't start the transfer";
        default:
            return `${moving ? "Moving" : "Copying"} ${subject}`;
    }
}

export function statusLine(view: TransferView): string {
    const { snapshot } = view;
    const files = `${formatCount(snapshot.done_files)} of ${plural(snapshot.total_files, "file")}`;
    if (snapshot.mode === "delete") {
        switch (snapshot.state) {
            case "planning":
                return "Looking through the selected folders…";
            case "paused":
                return `Paused · ${files}`;
            case "completed":
                return `${plural(snapshot.total_files, "file")} deleted from ${view.destination}`;
            case "completed_with_errors":
                return `${plural(snapshot.failed_files, "item")} couldn't be deleted and ${
                    snapshot.failed_files === 1 ? "is" : "are"
                } still there`;
            case "cancelled":
                return `Stopped after ${files}`;
            case "failed":
                return "Nothing was deleted";
            default:
                return snapshot.seconds_left === null
                    ? `Starting · ${files}`
                    : `${formatDuration(snapshot.seconds_left)} left · ${files}`;
        }
    }
    switch (snapshot.state) {
        case "planning":
            return "Looking through the selected folders…";
        case "paused":
            return `Paused · ${files}`;
        case "waiting_for_answer":
            return "Waiting for your choice";
        case "completed":
            return `${formatBytes(snapshot.total_bytes)} to ${view.destination}`;
        case "completed_with_errors":
            return `${plural(snapshot.failed_files, "file")} didn't transfer${
                snapshot.mode === "move"
                    ? snapshot.failed_files === 1
                        ? " and stayed where it was"
                        : " and stayed where they were"
                    : ""
            }`;
        case "cancelled":
            return `Stopped after ${files}`;
        case "failed":
            return "Nothing was transferred";
        default:
            return snapshot.seconds_left === null
                ? `Starting · ${files}`
                : `${formatDuration(snapshot.seconds_left)} left · ${files}`;
    }
}

export function describeFailure(failure: TransferFailure): string {
    switch (failure.reason.kind) {
        case "permission_denied":
            return "The server doesn't allow changes here";
        case "not_found":
            return "It was no longer there";
        case "authentication_failed":
            return "The sign-in was rejected";
        case "network":
            return `Connection problem (${failure.reason.detail})`;
        default:
            return failure.reason.detail;
    }
}

export function directionLabel(direction: TransferView["direction"]): string {
    return direction === "upload"
        ? "Upload"
        : direction === "download"
          ? "Download"
          : direction === "delete"
            ? "Delete"
            : "Transfer";
}

/** The chip naming what the card does. */
export function modeLabel(mode: TransferSnapshot["mode"]): string {
    return mode === "move" ? "Move" : mode === "delete" ? "Delete" : "Copy";
}

/** Progress from 0 to 100 for a logged transfer. */
export function entryPercent(entry: TransferHistoryEntry): number {
    if (entry.state === "completed") return 100;
    const ratio =
        entry.total_bytes > 0
            ? entry.done_bytes / entry.total_bytes
            : entry.total_files > 0
              ? (entry.done_files + entry.skipped_files) / entry.total_files
              : 0;
    return Math.min(100, Math.max(0, Math.floor(ratio * 100)));
}

export type StatusTone = "live" | "ok" | "warn" | "bad" | "quiet";

export function historyStatus(state: TransferHistoryEntry["state"]): {
    label: string;
    tone: StatusTone;
} {
    switch (state) {
        case "planning":
            return { label: "Getting ready", tone: "live" };
        case "running":
            return { label: "In progress", tone: "live" };
        case "paused":
            return { label: "Paused", tone: "quiet" };
        case "waiting_for_answer":
            return { label: "Waiting for your choice", tone: "warn" };
        case "completed":
            return { label: "Completed", tone: "ok" };
        case "completed_with_errors":
            return { label: "Completed with problems", tone: "warn" };
        case "cancelled":
            return { label: "Cancelled", tone: "quiet" };
        case "interrupted":
            return { label: "Interrupted", tone: "bad" };
        default:
            return { label: "Failed", tone: "bad" };
    }
}

function baseName(path: string): string {
    const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
    return parts[parts.length - 1] ?? path;
}

const VERBS: Record<
    TransferHistoryEntry["mode"],
    { doing: string; done: string; name: string }
> = {
    copy: { doing: "Copying", done: "Copied", name: "Copy" },
    move: { doing: "Moving", done: "Moved", name: "Move" },
    delete: { doing: "Deleting", done: "Deleted", name: "Delete" },
};

/** "Copied 3 items to Y:\\Filmer", "Deleting 2,000 items from Y:\\Old", and so on. */
export function historyTitle(entry: TransferHistoryEntry): string {
    const verbs = VERBS[entry.mode];
    const subject =
        entry.item_count === 1 && entry.items[0]
            ? baseName(entry.items[0])
            : `${formatCount(entry.item_count)} item${entry.item_count === 1 ? "" : "s"}`;
    const where = entry.destination
        ? ` ${entry.mode === "delete" ? "from" : "to"} ${entry.destination}`
        : "";
    switch (entry.state) {
        case "completed":
            return `${verbs.done} ${subject}${where}`;
        case "completed_with_errors":
            return `${verbs.done} ${subject}${where} with problems`;
        case "cancelled":
            return `${verbs.name} cancelled: ${subject}${where}`;
        case "interrupted":
            return `${verbs.name} interrupted: ${subject}${where}`;
        case "failed":
            return `Couldn't ${verbs.name.toLowerCase()} ${subject}${where}`;
        default:
            return `${verbs.doing} ${subject}${where}`;
    }
}

/** Connection, file counts and size on one line. */
export function historyDetail(entry: TransferHistoryEntry): string {
    const parts: string[] = [];
    if (entry.connection) parts.push(entry.connection);
    if (entry.total_files > 0) {
        parts.push(
            `${formatCount(entry.done_files)} of ${plural(entry.total_files, "file")}`,
        );
    }
    if (entry.mode !== "delete" && entry.total_bytes > 0) {
        parts.push(formatBytes(entry.total_bytes));
    }
    if (entry.skipped_files > 0) {
        parts.push(`${formatCount(entry.skipped_files)} skipped`);
    }
    if (entry.failed_files > 0) {
        parts.push(`${formatCount(entry.failed_files)} failed`);
    }
    return parts.join(" · ");
}

/** "12 s", "3 min 4 s", "1 h 2 min". */
export function formatElapsed(milliseconds: number): string {
    const seconds = Math.max(0, Math.round(milliseconds / 1000));
    if (seconds < 60) return `${seconds} s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes} min ${seconds % 60} s`;
    return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** The time a transfer started, in the user's own format. */
export function formatWhen(iso: string): string {
    const date = new Date(iso);
    return Number.isNaN(date.getTime())
        ? iso
        : date.toLocaleString(undefined, {
              month: "short",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
          });
}
