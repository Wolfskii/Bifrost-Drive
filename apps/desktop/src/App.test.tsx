import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "./App";

describe("App", () => {
    afterEach(() => cleanup());

    it("renders the connection workspace", () => {
        render(<App />);

        expect(
            screen.getByRole("heading", { name: "Your connections" }),
        ).toBeTruthy();
        expect(
            screen.getByRole("button", { name: /add connection/i }),
        ).toBeTruthy();
    });

    it("hides native mount controls when no filesystem integration is available", () => {
        render(<App />);

        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );

        expect(
            screen.getByRole("heading", { name: "Connect to Amazon S3" }),
        ).toBeTruthy();
        expect(screen.queryByRole("dialog")).toBeNull();
        expect(
            screen.queryByRole("combobox", { name: "Drive type" }),
        ).toBeNull();
        expect(
            screen.queryByRole("combobox", { name: "Windows drive" }),
        ).toBeNull();
        expect(
            screen.queryByRole("button", { name: "System default" }),
        ).toBeNull();
        expect(
            screen.queryByLabelText("Mount this location when Bifrost starts"),
        ).toBeNull();
    });

    it("shows provider-specific fields when the storage type changes", () => {
        render(<App />);
        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );

        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );
        fireEvent.click(screen.getByRole("option", { name: /SFTP server/i }));

        expect(
            screen.getByRole("heading", { name: "Connect to SFTP server" }),
        ).toBeTruthy();
        expect(screen.queryByLabelText("Known hosts file")).toBeNull();
        expect(screen.getByLabelText("Start path")).toBeTruthy();
        expect(
            screen.getByLabelText("Trust a new server key on first use"),
        ).toBeTruthy();
        expect(
            (
                screen.getByLabelText(
                    "Trust a new server key on first use",
                ) as HTMLInputElement
            ).checked,
        ).toBe(true);
        expect(screen.getByLabelText("Password")).toBeTruthy();
        expect(screen.queryByLabelText("Private key path")).toBeNull();

        fireEvent.click(
            screen.getByRole("combobox", { name: /authentication/i }),
        );
        fireEvent.click(screen.getByRole("option", { name: /Private key/i }));

        expect(screen.getByLabelText("Private key path")).toBeTruthy();
        expect(screen.queryByLabelText("Password")).toBeNull();
    });

    it("offers S3 presets and a working Google Drive connection", () => {
        render(<App />);
        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );
        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );

        const googleDrive = screen.getByRole("option", {
            name: /Google Drive/i,
        }) as HTMLButtonElement;
        expect(googleDrive.disabled).toBe(false);

        fireEvent.click(googleDrive);
        expect(
            screen.getByRole("heading", { name: "Connect to Google Drive" }),
        ).toBeTruthy();
        expect(
            screen.getByRole("button", { name: "Sign in with Google" }),
        ).toBeTruthy();
        expect(screen.queryByLabelText("Endpoint")).toBeNull();
        expect(screen.queryByLabelText("Google OAuth client ID")).toBeNull();

        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );
        fireEvent.click(screen.getByRole("option", { name: /Cloudflare R2/i }));
        expect(
            screen.getByRole("heading", { name: "Connect to Cloudflare R2" }),
        ).toBeTruthy();
        expect(screen.getByLabelText("Bucket")).toBeTruthy();
    });

    it("shows the start-minimized tray setting", () => {
        render(<App />);
        fireEvent.click(screen.getByRole("button", { name: "Settings" }));

        const checkbox = screen.getByRole("checkbox", {
            name: /Start minimized in the notification tray/i,
        }) as HTMLInputElement;
        fireEvent.click(
            screen.getByText("Start minimized in the notification tray"),
        );

        expect(checkbox.checked).toBe(false);
    });

    it("shows update controls in settings", () => {
        render(<App />);
        fireEvent.click(screen.getByRole("button", { name: "Settings" }));

        expect(
            screen.getByRole("button", { name: "Check for updates" }),
        ).toBeTruthy();
        expect(
            screen.getByRole("checkbox", { name: "Show update popups" }),
        ).toBeTruthy();
        expect(
            screen.getByText("No update is currently available."),
        ).toBeTruthy();
    });
});
