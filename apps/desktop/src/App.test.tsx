import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
    App,
    ConnectionProviderIcon,
    parseReleaseNotes,
    providerSelectionForKind,
    webUiUrlForConnection,
} from "./App";

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

    it("links users to the privacy policy from the app", () => {
        render(<App />);
        fireEvent.click(screen.getByRole("button", { name: "Settings" }));

        const privacyLink = screen.getByRole("link", {
            name: "Privacy Policy",
        });
        expect(privacyLink.getAttribute("href")).toBe(
            "https://bifrost.webble.se/privacy/",
        );
    });

    it("uses provider icons for saved connections", () => {
        const { rerender } = render(
            <ConnectionProviderIcon kind="GoogleDrive" />,
        );
        expect(screen.getByLabelText("Google Drive")).toBeTruthy();

        rerender(<ConnectionProviderIcon kind="S3" />);
        expect(screen.getByLabelText("S3 object storage")).toBeTruthy();
    });

    it("maps saved providers to connection wizard options", () => {
        expect(providerSelectionForKind("GoogleDrive")).toBe("google-drive");
        expect(providerSelectionForKind("Immich")).toBe("immich");
        expect(providerSelectionForKind("Sftp")).toBe("SFTP");
        expect(providerSelectionForKind("WebDav")).toBe("WebDAV");
    });

    it("maps browser-capable providers to their web interfaces", () => {
        expect(
            webUiUrlForConnection({
                kind: "Immich",
                endpoint: "https://images.example.com",
            }),
        ).toBe("https://images.example.com");
        expect(
            webUiUrlForConnection({
                kind: "GoogleDrive",
                endpoint: "https://www.googleapis.com/drive/v3",
            }),
        ).toBe("https://drive.google.com/drive/my-drive");
        expect(
            webUiUrlForConnection({
                kind: "GooglePhotos",
                endpoint: "https://photoslibrary.googleapis.com/v1",
            }),
        ).toBe("https://photos.google.com/");
        expect(
            webUiUrlForConnection({
                kind: "Sftp",
                endpoint: "sftp://files.example.com:22",
            }),
        ).toBeNull();
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
            screen.getByRole("combobox", { name: /storage type/i }),
        );
        fireEvent.click(screen.getByRole("option", { name: /WebDAV server/i }));

        expect(
            screen.getByRole("heading", { name: "Connect to WebDAV server" }),
        ).toBeTruthy();
        expect(screen.getByLabelText("Start path")).toBeTruthy();

        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );
        fireEvent.click(screen.getByRole("option", { name: /SFTP server/i }));

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
        expect(
            (
                screen.getByRole("checkbox", {
                    name: "Open Google Workspace files in OS native apps",
                }) as HTMLInputElement
            ).checked,
        ).toBe(true);
        expect(
            (
                screen.getByRole("button", {
                    name: "Mount drive",
                }) as HTMLButtonElement
            ).disabled,
        ).toBe(true);
        expect(screen.queryByLabelText("Endpoint")).toBeNull();
        expect(screen.queryByLabelText("Google OAuth client ID")).toBeNull();
        expect(
            screen.queryByLabelText("Shared Drive ID (optional)"),
        ).toBeNull();

        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );
        fireEvent.click(screen.getByRole("option", { name: /Cloudflare R2/i }));
        expect(
            screen.getByRole("heading", { name: "Connect to Cloudflare R2" }),
        ).toBeTruthy();
        expect(screen.getByLabelText("Bucket")).toBeTruthy();
    });

    it("offers an official Google Photos connection", () => {
        render(<App />);
        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );
        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );

        const googlePhotos = screen.getByRole("option", {
            name: /Google Photos/i,
        }) as HTMLButtonElement;
        expect(googlePhotos.disabled).toBe(false);

        fireEvent.click(googlePhotos);
        expect(
            screen.getByRole("heading", { name: "Connect to Google Photos" }),
        ).toBeTruthy();
        expect(
            screen.getByRole("button", { name: "Sign in with Google" }),
        ).toBeTruthy();
        expect(
            screen.getByText(/only media and albums created by Bifrost/i),
        ).toBeTruthy();
        expect(screen.queryByLabelText("Endpoint")).toBeNull();
        expect(screen.queryByLabelText("Username")).toBeNull();
        expect(screen.queryByLabelText("Password")).toBeNull();
        expect(
            (
                screen.getByLabelText(
                    "Legacy Google Drive folder path",
                ) as HTMLInputElement
            ).value,
        ).toBe("Fritid/Google Foto");
    });

    it("offers Immich with API key and email authentication", () => {
        render(<App />);
        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );
        fireEvent.click(
            screen.getByRole("combobox", { name: /storage type/i }),
        );

        const immich = screen.getByRole("option", {
            name: /Immich/i,
        }) as HTMLButtonElement;
        expect(immich.disabled).toBe(false);

        fireEvent.click(immich);
        expect(
            screen.getByRole("heading", { name: "Connect to Immich" }),
        ).toBeTruthy();
        expect(screen.getByLabelText("Endpoint").getAttribute("type")).toBe(
            "text",
        );
        expect(screen.getByLabelText("API key")).toBeTruthy();
        expect(screen.queryByLabelText("Email")).toBeNull();

        fireEvent.click(
            screen.getByRole("combobox", { name: /authentication/i }),
        );
        fireEvent.click(
            screen.getByRole("option", { name: /Email and password/i }),
        );

        expect(screen.getByLabelText("Email")).toBeTruthy();
        expect(screen.getByLabelText("Password")).toBeTruthy();
        expect(screen.queryByLabelText("API key")).toBeNull();
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

    it("parses release-note markdown into presentable blocks", () => {
        expect(
            parseReleaseNotes(
                "## Changes\n\n- feat: add Google Drive (abc1234)\n- fix: mount flow (def5678)",
            ),
        ).toEqual([
            { kind: "heading", text: "Changes" },
            {
                kind: "list",
                items: [
                    "feat: add Google Drive (abc1234)",
                    "fix: mount flow (def5678)",
                ],
            },
        ]);
    });
});
