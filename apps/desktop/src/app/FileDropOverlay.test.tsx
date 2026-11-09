import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { FileDropOverlay } from "./FileDropOverlay";
import { toastStore } from "./toast";

type EchoTest = { emit: (event: string, payload: unknown) => void };

function emit(event: string, payload: unknown): void {
  (globalThis as unknown as { __echoTest: EchoTest }).__echoTest.emit(event, payload);
}

const overlay = () => screen.queryByRole("status", { name: "松手即播放" });

describe("Window file-drop feedback", () => {
  it("appears when a drag enters and disappears when it leaves or drops", async () => {
    render(<FileDropOverlay />);
    expect(overlay()).not.toBeInTheDocument();

    await act(async () => {
      emit("app://file-drop-state", { active: true });
    });
    expect(overlay()).toBeInTheDocument();
    expect(screen.getByText("松手即播放")).toBeInTheDocument();
    // The hint must never steal hits from the titlebar drag strip, the Windows
    // caption buttons or an open popover.
    expect(screen.getByTestId("file-drop-overlay")).toHaveStyle("pointer-events: none");

    await act(async () => {
      emit("app://file-drop-state", { active: false });
    });
    expect(overlay()).not.toBeInTheDocument();
  });

  it("shows a readable, error-styled rejection for unsupported drops", async () => {
    render(<FileDropOverlay />);
    await act(async () => {
      emit("app://file-drop-rejection", {
        message: "不支持拖入文件夹（1 个），请逐个拖入音频文件",
      });
    });
    expect(toastStore.get()?.message).toBe("不支持拖入文件夹（1 个），请逐个拖入音频文件");
    expect(toastStore.get()?.error).toBe(true);
  });

  it("ignores malformed drop state instead of guessing", async () => {
    render(<FileDropOverlay />);
    await act(async () => {
      emit("app://file-drop-state", { active: "yes" });
    });
    expect(overlay()).not.toBeInTheDocument();
  });

  it("removes motion but keeps the state change under prefers-reduced-motion", async () => {
    const css = readFileSync(resolve(process.cwd(), "src/styles/shell.css"), "utf8");
    expect(css).toContain(".file-drop-overlay");
    expect(css).toContain("pointer-events: none");
    expect(css).toContain("prefers-reduced-motion: reduce");

    render(<FileDropOverlay />);
    await act(async () => {
      emit("app://file-drop-state", { active: true });
    });
    expect(overlay()).toBeInTheDocument();
  });
});
