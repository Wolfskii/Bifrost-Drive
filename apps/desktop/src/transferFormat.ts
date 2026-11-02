import type { TransferFailure, TransferSnapshot, TransferView } from "./api";

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
