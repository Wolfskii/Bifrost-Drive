import { describe, expect, it } from "vitest";
import { snapshot, view } from "./transferTestData";
import {
    cardTitle,
    describeFailure,
    formatBytes,
    formatDuration,
    percentDone,
    segmentFills,
    statusLine,
} from "./transferFormat";

describe("transfer formatting", () => {
    it("formats sizes the way Explorer does", () => {
        expect(formatBytes(0)).toBe("0 B");
        expect(formatBytes(1536)).toBe("1.5 KB");
        expect(formatBytes(41.2 * 1024 * 1024)).toBe("41.2 MB");
        expect(formatBytes(250 * 1024 ** 3)).toBe("250 GB");
    });

    it("words the time left without false precision", () => {
        expect(formatDuration(20)).toBe("less than a minute");
        expect(formatDuration(150)).toBe("about 3 min");
        expect(formatDuration(3600)).toBe("about 1 h");
        expect(formatDuration(3900)).toBe("about 1 h 5 min");
    });

    it("measures progress by bytes, falling back to files for empty files", () => {
        expect(percentDone(snapshot())).toBe(71);
        expect(
            percentDone(
                snapshot({ total_bytes: 0, total_files: 4, done_files: 1 }),
            ),
        ).toBe(25);
        expect(percentDone(snapshot({ done_bytes: 5000 }))).toBe(100);
        expect(
            percentDone(snapshot({ state: "completed", done_bytes: 0 })),
        ).toBe(100);
    });

    it("fills the segmented bar from the left with one partial segment", () => {
        const fills = segmentFills(71, 28);

        expect(fills).toHaveLength(28);
        expect(fills.filter((fill) => fill === 1)).toHaveLength(19);
        expect(fills[19]).toBeCloseTo(0.88, 2);
        expect(fills.slice(20).every((fill) => fill === 0)).toBe(true);
        expect(segmentFills(0).every((fill) => fill === 0)).toBe(true);
        expect(segmentFills(100).every((fill) => fill === 1)).toBe(true);
    });

    it("titles cards by what is happening", () => {
        expect(cardTitle(view())).toBe("Copying 2,000 files");
        expect(cardTitle(view({ mode: "move" }))).toBe("Moving 2,000 files");
        expect(
            cardTitle(
                view({ total_files: 1 }, { items: 1, first_item: "a.mkv" }),
            ),
        ).toBe("Copying a.mkv");
        expect(cardTitle(view({ state: "completed", mode: "move" }))).toBe(
            "Moved 2,000 files",
        );
        expect(
            cardTitle(
                view({ state: "completed_with_errors", done_files: 1999 }),
            ),
        ).toBe("Copied 1,999 of 2,000 files");
        expect(cardTitle(view({ state: "cancelled" }))).toBe(
            "Transfer cancelled",
        );
    });

    it("words the status line for each state", () => {
        expect(statusLine(view({ seconds_left: 150 }))).toBe(
            "about 3 min left · 1,204 of 2,000 files",
        );
        expect(statusLine(view({ state: "paused" }))).toBe(
            "Paused · 1,204 of 2,000 files",
        );
        expect(
            statusLine(
                view({
                    state: "completed_with_errors",
                    failed_files: 1,
                    mode: "move",
                }),
            ),
        ).toBe("1 file didn't transfer and stayed where it was");
    });

    it("explains failures in plain words", () => {
        expect(
            describeFailure({
                name: "a.mkv",
                reason: { kind: "permission_denied" },
            }),
        ).toBe("The server doesn't allow changes here");
        expect(
            describeFailure({
                name: "a.mkv",
                reason: { kind: "network", detail: "connection reset" },
            }),
        ).toBe("Connection problem (connection reset)");
    });
});
