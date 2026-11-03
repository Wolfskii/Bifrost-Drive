import { describe, expect, it } from "vitest";
import {
    entryPercent,
    formatElapsed,
    historyDetail,
    historyStatus,
    historyTitle,
} from "./transferFormat";
import { historyEntry } from "./transferTestData";

describe("the transfer log wording", () => {
    it("titles each state of a transfer", () => {
        const base = historyEntry();
        const title = (overrides: Partial<typeof base>) =>
            historyTitle({ ...base, ...overrides });

        expect(title({})).toBe("Copied 3 items to Y:\\Filmer");
        expect(title({ mode: "move" })).toBe("Moved 3 items to Y:\\Filmer");
        expect(title({ state: "running" })).toBe(
            "Copying 3 items to Y:\\Filmer",
        );
        expect(title({ item_count: 1, items: ["C:\\Videos\\a.mkv"] })).toBe(
            "Copied a.mkv to Y:\\Filmer",
        );
        expect(title({ mode: "delete", state: "completed" })).toBe(
            "Deleted 3 items from Y:\\Filmer",
        );
        expect(title({ state: "failed", destination: "" })).toBe(
            "Couldn't copy 3 items",
        );
        expect(title({ state: "cancelled" })).toBe(
            "Copy cancelled: 3 items to Y:\\Filmer",
        );
    });

    it("names a state and its tone", () => {
        expect(historyStatus("completed")).toEqual({
            label: "Completed",
            tone: "ok",
        });
        expect(historyStatus("running").tone).toBe("live");
        expect(historyStatus("failed").tone).toBe("bad");
        expect(historyStatus("interrupted").label).toBe("Interrupted");
    });

    it("measures progress by bytes, or by files when there are none", () => {
        expect(
            entryPercent(
                historyEntry({
                    state: "running",
                    total_bytes: 200,
                    done_bytes: 50,
                }),
            ),
        ).toBe(25);
        expect(
            entryPercent(
                historyEntry({
                    state: "running",
                    total_bytes: 0,
                    total_files: 4,
                    done_files: 1,
                    skipped_files: 1,
                }),
            ),
        ).toBe(50);
        expect(entryPercent(historyEntry({ state: "completed" }))).toBe(100);
    });

    it("formats elapsed time", () => {
        expect(formatElapsed(900)).toBe("1 s");
        expect(formatElapsed(184_000)).toBe("3 min 4 s");
        expect(formatElapsed(3_720_000)).toBe("1 h 2 min");
    });

    it("summarises counts on one line", () => {
        expect(
            historyDetail(historyEntry({ skipped_files: 1, failed_files: 2 })),
        ).toBe("Yggdrasil SFTP · 3 of 3 files · 3.0 MB · 1 skipped · 2 failed");
    });
});
