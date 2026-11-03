import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TransferLog } from "./TransferLog";
import { historyEntry } from "./transferTestData";

afterEach(cleanup);

function show(entries = [historyEntry()]) {
    const onClear = vi.fn();
    const onOpenWindow = vi.fn();
    render(
        <TransferLog
            entries={entries}
            onClear={onClear}
            onOpenWindow={onOpenWindow}
        />,
    );
    return { onClear, onOpenWindow };
}

describe("TransferLog", () => {
    it("tells the user how to start a transfer when the log is empty", () => {
        show([]);

        expect(screen.getByText(/No transfers yet/)).toBeTruthy();
        expect(screen.queryByText("Clear finished")).toBeNull();
        expect(screen.queryByText("Show transfer window")).toBeNull();
    });

    it("shows what was transferred, where, how it went and how long it took", () => {
        show();

        expect(screen.getByText("Copied 3 items to Y:\\Filmer")).toBeTruthy();
        expect(screen.getByText("Completed")).toBeTruthy();
        expect(screen.getByText(/Yggdrasil SFTP · 3 of 3 files/)).toBeTruthy();
        expect(screen.getByText(/took 3 min 4 s/)).toBeTruthy();
    });

    it("lists the selected files and says how many were left out", () => {
        show([historyEntry({ item_count: 250 })]);

        fireEvent.click(screen.getByText("Selected 250 items"));

        expect(screen.getByText("C:\\Videos\\a.mkv")).toBeTruthy();
        expect(screen.getByText("…and 247 more")).toBeTruthy();
    });

    it("shows a running transfer with its progress, speed and current file", () => {
        const { onOpenWindow } = show([
            historyEntry({
                state: "running",
                finished_at: null,
                total_bytes: 1000,
                done_bytes: 710,
                done_files: 1,
                live: {
                    transfer_id: 4,
                    bytes_per_second: 41 * 1024 * 1024,
                    files_per_second: 0,
                    seconds_left: 150,
                    current: ["clips/a.mkv", "clips/b.mkv"],
                },
            }),
        ]);

        const bar = screen.getByRole("progressbar");
        expect(bar.getAttribute("aria-valuenow")).toBe("71");
        expect(
            screen.getByText("71% · 41.0 MB/s · about 3 min left"),
        ).toBeTruthy();
        expect(screen.getByText("clips/a.mkv and 1 more")).toBeTruthy();
        expect(screen.getByText("Copying 3 items to Y:\\Filmer")).toBeTruthy();

        fireEvent.click(screen.getByText("Show transfer window"));
        expect(onOpenWindow).toHaveBeenCalledOnce();
    });

    it("explains problems in plain words, next to the files they concern", () => {
        show([
            historyEntry({
                state: "completed_with_errors",
                done_files: 1,
                failed_files: 2,
                failures: [
                    { name: "a.mkv", reason: { kind: "permission_denied" } },
                    {
                        name: "b.mkv",
                        reason: { kind: "network", detail: "connection reset" },
                    },
                ],
            }),
        ]);

        expect(screen.getByText("Completed with problems")).toBeTruthy();
        expect(screen.getByText("2 problems")).toBeTruthy();
        expect(
            screen.getByText("The server doesn't allow changes here"),
        ).toBeTruthy();
        expect(
            screen.getByText("Connection problem (connection reset)"),
        ).toBeTruthy();
    });

    it("words a delete and shows files per second while it runs", () => {
        show([
            historyEntry({
                mode: "delete",
                direction: "delete",
                state: "running",
                finished_at: null,
                total_bytes: 0,
                total_files: 2000,
                done_files: 500,
                item_count: 1,
                items: ["Y:\\Old\\photos"],
                live: {
                    transfer_id: 2,
                    bytes_per_second: 0,
                    files_per_second: 1500,
                    seconds_left: null,
                    current: [],
                },
            }),
        ]);

        expect(
            screen.getByText("Deleting photos from Y:\\Filmer"),
        ).toBeTruthy();
        expect(screen.getByText("25% · 1,500 files/s")).toBeTruthy();
    });

    it("marks transfers that were cut short by the app closing", () => {
        show([historyEntry({ state: "interrupted" })]);

        expect(screen.getByText("Interrupted")).toBeTruthy();
        expect(
            screen.getByText("Copy interrupted: 3 items to Y:\\Filmer"),
        ).toBeTruthy();
    });

    it("offers to clear only when something has finished", () => {
        const { onClear } = show();

        fireEvent.click(screen.getByText("Clear finished"));

        expect(onClear).toHaveBeenCalledOnce();
    });
});
