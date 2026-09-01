import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App shell", () => {
  it("renders the echo shell and shows the first-launch choose-root view when no library is configured", () => {
    render(<App />);
    expect(screen.getByTestId("echo-shell")).toBeInTheDocument();
    // The default library_status mock reports unconfigured, so the shell must
    // show the initialize view — never a working library it doesn't have.
    expect(screen.getByTestId("choose-root")).toBeInTheDocument();
    expect(screen.queryByTestId("sidebar")).not.toBeInTheDocument();
  });
});
