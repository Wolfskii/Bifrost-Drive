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

    it("opens the S3 connection wizard", () => {
        render(<App />);

        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );

        expect(
            screen.getByRole("heading", { name: "Connect to S3" }),
        ).toBeTruthy();
        expect(
            screen.getByLabelText("Mount this drive when Bifrost starts"),
        ).toBeTruthy();
    });

    it("shows provider-specific fields when the storage type changes", () => {
        render(<App />);
        fireEvent.click(
            screen.getByRole("button", { name: /add connection/i }),
        );

        fireEvent.change(
            screen.getByRole("combobox", { name: /storage type/i }),
            {
                target: { value: "SFTP" },
            },
        );

        expect(screen.getByRole("heading", { name: "Connect to SFTP" })).toBeTruthy();
        expect(screen.queryByLabelText("Known hosts file")).toBeNull();
        expect(screen.getByLabelText("Start path")).toBeTruthy();
        expect(
            screen.getByLabelText("Trust a new server key on first use"),
        ).toBeTruthy();
        expect(screen.getByLabelText("Password")).toBeTruthy();
        expect(screen.queryByLabelText("Private key path")).toBeNull();

        fireEvent.change(
            screen.getByRole("combobox", { name: /authentication/i }),
            {
                target: { value: "private_key" },
            },
        );

        expect(screen.getByLabelText("Private key path")).toBeTruthy();
        expect(screen.queryByLabelText("Password")).toBeNull();
    });
});
