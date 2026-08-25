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
            screen.getByRole("dialog", { name: "Connect to S3" }),
        ).toBeTruthy();
    });
});
