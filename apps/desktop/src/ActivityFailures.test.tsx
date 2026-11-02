import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", async (importOriginal) => ({
    ...(await importOriginal<typeof import("./api")>()),
    listActivity: vi.fn().mockResolvedValue([
        {
            id: "1",
            kind: "drive_upload_failed",
            remote_path:
                "“video.mkv” wasn't uploaded. Yggdrasil SFTP doesn't allow changes in Y:\\Filmer.",
            status: "failed",
            created_at: "2026-11-02T10:12:41Z",
        },
    ]),
}));

const { App } = await import("./App");

describe("drive failures in activity", () => {
    afterEach(() => cleanup());

    it("shows the failure in plain words and offers the recovered uploads", async () => {
        render(<App />);
        fireEvent.click(screen.getByRole("button", { name: "Activity" }));

        expect(
            await screen.findByText("Upload failed, data kept on this PC"),
        ).toBeTruthy();
        expect(
            screen.getByText(/doesn't allow changes in Y:\\Filmer/),
        ).toBeTruthy();
        expect(
            screen.getByRole("button", { name: /open recovered uploads/i }),
        ).toBeTruthy();
    });
});
