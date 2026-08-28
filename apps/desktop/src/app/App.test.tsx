import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App shell", () => {
  it("renders the echo shell and reports an unavailable boot state", () => {
    render(<App />);
    expect(screen.getByTestId("echo-shell")).toBeInTheDocument();
    expect(screen.getByTestId("boot-state")).toHaveTextContent("unavailable");
  });
});
