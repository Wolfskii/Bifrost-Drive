import {
    act,
    cleanup,
    fireEvent,
    render,
    screen,
    waitFor,
    within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TransferView } from "./api";
import { view } from "./transferTestData";

const api = vi.hoisted(() => ({
    listTransfers: vi.fn(),
    subscribeTransfers: vi.fn(),
    pauseTransfer: vi.fn(),
    resumeTransfer: vi.fn(),
    cancelTransfer: vi.fn(),
    dismissTransfer: vi.fn(),
    answerTransferConflict: vi.fn(),
}));

vi.mock("./api", () => api);

const { TransferWindow } = await import("./TransferWindow");

let push: (update: TransferView) => void = () => undefined;

async function openWindow(initial: TransferView[]) {
    api.listTransfers.mockResolvedValue(initial);
    render(<TransferWindow />);
    await waitFor(() => expect(api.subscribeTransfers).toHaveBeenCalled());
}

describe("TransferWindow", () => {
    beforeEach(() => {
        for (const mock of Object.values(api)) mock.mockReset();
        api.subscribeTransfers.mockImplementation(
            async (callback: (update: TransferView) => void) => {
                push = callback;
                return () => undefined;
            },
        );
        for (const name of [
            "pauseTransfer",
            "resumeTransfer",
            "cancelTransfer",
            "dismissTransfer",
            "answerTransferConflict",
        ] as const) {
            api[name].mockResolvedValue(undefined);
        }
    });
    afterEach(() => cleanup());

    it("shows what the transfer is, how far along it is, and how fast", async () => {
        await openWindow([
            view({
                mode: "move",
                bytes_per_second: 41 * 1024 * 1024,
                seconds_left: 150,
                current: ["clips/2026-10-31.mkv", "clips/b.mkv"],
            }),
        ]);

        const chips = await screen.findByRole("list", {
            name: "Transfer details",
        });
        expect(within(chips).getByText("Yggdrasil SFTP")).toBeTruthy();
        expect(within(chips).getByText("Move")).toBeTruthy();
        expect(within(chips).getByText("Upload")).toBeTruthy();
        expect(
            screen.getByRole("heading", { name: "Moving 2,000 files" }),
        ).toBeTruthy();
        expect(screen.getByText("71%")).toBeTruthy();
        expect(screen.getByText("↗ 41.0 MB/s")).toBeTruthy();
        expect(screen.getByText("current speed")).toBeTruthy();
        expect(
            screen.getByText("about 3 min left · 1,204 of 2,000 files"),
        ).toBeTruthy();
        expect(
            screen.getByText(/clips\/2026-10-31\.mkv and 1 more/),
        ).toBeTruthy();

        const bar = screen.getByRole("progressbar");
        expect(bar.getAttribute("aria-valuenow")).toBe("71");
        expect(bar.children).toHaveLength(28);
        expect(
            Array.from(bar.children).filter(
                (segment) => segment.getAttribute("data-on") === "true",
            ),
        ).toHaveLength(19);
    });

    it("follows live updates without reloading", async () => {
        await openWindow([view({ done_bytes: 100 })]);
        await screen.findByText("10%");

        act(() => push(view({ done_bytes: 900 })));

        expect(await screen.findByText("90%")).toBeTruthy();
    });

    it("asks what to do about a name clash and passes the choice on", async () => {
        await openWindow([
            view({
                state: "waiting_for_answer",
                conflict: {
                    name: "clips/a.mkv",
                    source_bytes: 3 * 1024 * 1024,
                    destination_bytes: 2 * 1024 * 1024,
                },
            }),
        ]);

        expect(
            await screen.findByText(/“clips\/a\.mkv” already exists/),
        ).toBeTruthy();
        fireEvent.click(screen.getByLabelText("Do this for every name clash"));
        fireEvent.click(screen.getByRole("button", { name: "Keep both" }));

        expect(api.answerTransferConflict).toHaveBeenCalledWith(
            1,
            "keep_both",
            true,
        );
    });

    it("pauses, resumes and cancels from the options menu", async () => {
        await openWindow([view()]);
        const open = async () =>
            fireEvent.click(
                await screen.findByRole("button", { name: "Transfer options" }),
            );

        await open();
        fireEvent.click(screen.getByRole("menuitem", { name: "Pause" }));
        expect(api.pauseTransfer).toHaveBeenCalledWith(1);

        await open();
        fireEvent.click(
            screen.getByRole("menuitem", { name: "Cancel transfer" }),
        );
        expect(api.cancelTransfer).toHaveBeenCalledWith(1);

        act(() => push(view({ state: "paused" })));
        await open();
        fireEvent.click(
            await screen.findByRole("menuitem", { name: "Resume" }),
        );
        expect(api.resumeTransfer).toHaveBeenCalledWith(1);
    });

    it("lists what failed in plain words and lets a finished transfer be removed", async () => {
        await openWindow([
            view({
                state: "completed_with_errors",
                mode: "move",
                done_files: 1,
                failed_files: 1,
                failures: [
                    {
                        name: "clips/a.mkv",
                        reason: { kind: "permission_denied" },
                    },
                ],
            }),
        ]);

        expect(await screen.findByText("! 1 failed")).toBeTruthy();
        fireEvent.click(screen.getByText("1 problem"));
        expect(screen.getByText("clips/a.mkv")).toBeTruthy();
        expect(
            screen.getByText("The server doesn't allow changes here"),
        ).toBeTruthy();

        fireEvent.click(
            screen.getByRole("button", { name: "Transfer options" }),
        );
        fireEvent.click(
            screen.getByRole("menuitem", { name: "Remove from list" }),
        );

        expect(api.dismissTransfer).toHaveBeenCalledWith(1);
        await waitFor(() =>
            expect(screen.queryByText("! 1 failed")).toBeNull(),
        );
    });

    it("explains how to start a transfer when there are none", async () => {
        await openWindow([]);

        expect(await screen.findByText(/No transfers yet/)).toBeTruthy();
    });

    it("tells the user when a control did not work", async () => {
        api.pauseTransfer.mockRejectedValue(new Error("That transfer is gone"));
        await openWindow([view()]);

        fireEvent.click(
            await screen.findByRole("button", { name: "Transfer options" }),
        );
        fireEvent.click(screen.getByRole("menuitem", { name: "Pause" }));

        expect((await screen.findByRole("alert")).textContent).toBe(
            "That transfer is gone",
        );
    });
});
