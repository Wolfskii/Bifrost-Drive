import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App", () => {
    it("renders the connection workspace", () => {
        render(<App />);

        expect(
            screen.getByRole("heading", { name: "Your connections" }),
        ).toBeTruthy();
        expect(
            screen.getByRole("button", { name: /add connection/i }),
        ).toBeTruthy();
    });
});
