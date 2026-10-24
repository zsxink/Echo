/**
 * WindowControls (issue #76) — the Windows-only caption trio.
 *
 * `@tauri-apps/api/window` is mocked at the module boundary: the component
 * goes through `bridge`'s window facade, which delegates to `getCurrentWindow`
 * from that module. The mock records the action calls so each test can assert
 * the wiring and the glyph state without a real WebView.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { WindowControls } from "./WindowControls";

const windowCalls: string[] = [];
let maximized = false;
const resizeListeners = new Set<() => void>();

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(async () => {
      windowCalls.push("minimize");
    }),
    toggleMaximize: vi.fn(async () => {
      windowCalls.push("toggle_maximize");
      maximized = !maximized;
      for (const listener of resizeListeners) listener();
    }),
    close: vi.fn(async () => {
      windowCalls.push("close");
    }),
    isMaximized: vi.fn(async () => maximized),
    listen: vi.fn(async (_event: string, handler: () => void) => {
      resizeListeners.add(handler);
      return () => resizeListeners.delete(handler);
    }),
  }),
}));

function setPlatform(platform: string | undefined): void {
  if (platform === undefined) {
    delete document.documentElement.dataset.platform;
  } else {
    document.documentElement.dataset.platform = platform;
  }
}

describe("WindowControls", () => {
  beforeEach(() => {
    windowCalls.length = 0;
    maximized = false;
    resizeListeners.clear();
    setPlatform(undefined);
  });

  it("renders nothing outside the Windows platform", () => {
    for (const platform of ["macos", "linux", "web", undefined]) {
      setPlatform(platform);
      const { unmount } = render(<WindowControls />);
      expect(screen.queryByTestId("window-controls")).not.toBeInTheDocument();
      unmount();
    }
  });

  it("renders the caption trio on Windows", () => {
    setPlatform("windows");
    render(<WindowControls />);
    expect(screen.getByTestId("window-controls")).toBeInTheDocument();
    expect(screen.getByTestId("window-minimize")).toBeInTheDocument();
    expect(screen.getByTestId("window-toggle-maximize")).toBeInTheDocument();
    expect(screen.getByTestId("window-close")).toBeInTheDocument();
  });

  it("wires the three buttons to the window facade actions", async () => {
    setPlatform("windows");
    render(<WindowControls />);
    fireEvent.click(screen.getByTestId("window-minimize"));
    fireEvent.click(screen.getByTestId("window-close"));
    await waitFor(() => expect(windowCalls).toEqual(["minimize", "close"]));
  });

  it("tracks the real maximized state through resize events", async () => {
    setPlatform("windows");
    render(<WindowControls />);
    // Not maximized at boot: the caption reads 最大化.
    await waitFor(() =>
      expect(screen.getByTestId("window-toggle-maximize")).toHaveAttribute("aria-label", "最大化"),
    );
    fireEvent.click(screen.getByTestId("window-toggle-maximize"));
    await waitFor(() => expect(windowCalls).toContain("toggle_maximize"));
    // The stub toggles its state and fires the resize listener, so the glyph
    // follows the *window's* state, not a local guess (native double-click on
    // the drag strip changes it outside this component too).
    await waitFor(() =>
      expect(screen.getByTestId("window-toggle-maximize")).toHaveAttribute("aria-label", "还原"),
    );
    fireEvent.click(screen.getByTestId("window-toggle-maximize"));
    await waitFor(() =>
      expect(screen.getByTestId("window-toggle-maximize")).toHaveAttribute("aria-label", "最大化"),
    );
  });

  it("App renders the controls only when the platform is Windows", async () => {
    const { App } = await import("./App");
    const { unmount } = render(<App />);
    expect(screen.queryByTestId("window-controls")).not.toBeInTheDocument();
    unmount();
    setPlatform("windows");
    const { unmount: unmountWindows } = render(<App />);
    expect(screen.getByTestId("window-controls")).toBeInTheDocument();
    unmountWindows();
    setPlatform(undefined);
  });
});
