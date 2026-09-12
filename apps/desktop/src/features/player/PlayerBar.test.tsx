/**
 * PlayerBar tests — temporary playback item identification and import (task 11.7).
 *
 * Verifies:
 *  - A "临时" badge is shown when the current entry is a session-only temporary
 *    item (currentSongId is null, currentCanImport is true).
 *  - An "导入到资料库" button appears only for temporary items.
 *  - The import button sends the `import_current_temporary_file` command.
 *  - Library songs (with a currentSongId) do NOT show the import button or badge.
 */

import { render, screen, fireEvent } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { PlayerBar } from "./PlayerBar";
import { playerStore } from "../../player/playerStore";
import type { UiPlayerSnapshot } from "../../player/playerStore";

// Bridge calls are mocked globally by setup.ts; capture them for assertion.
let capturedInvoke: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  // Get a reference to the mocked invoke so we can inspect calls.
  const tauriCore = await import("@tauri-apps/api/core");
  capturedInvoke = (tauriCore as unknown as { invoke: ReturnType<typeof vi.fn> }).invoke;
  capturedInvoke.mockClear();

  // Reset the player store to the stopped/empty state.
  playerStore.publish({
    state: "stopped",
    position: null,
    duration: null,
    volume: 1,
    muted: false,
    currentQueueEntryId: null,
    currentSongId: null,
    queueLen: 0,
    mode: "sequential",
    currentTitle: null,
    currentCanImport: false,
    queue: [],
  });
  playerStore.setQueueOpen(false);
  playerStore.setImmersiveOpen(false);
  playerStore.setFocusOpen(false);
});

/** Helper: publish a snapshot and render the bar. */
function renderWithSnapshot(snapshot: Partial<UiPlayerSnapshot> = {}) {
  const full: UiPlayerSnapshot = {
    state: "playing",
    position: 30,
    duration: 120,
    volume: 0.8,
    muted: false,
    currentQueueEntryId: "entry-1",
    currentSongId: null,
    queueLen: 1,
    mode: "sequential",
    currentTitle: "晴天.mp3",
    currentCanImport: true,
    queue: [
      {
        entryId: "entry-1",
        songId: null,
        title: "晴天.mp3",
        isCurrent: true,
        failed: false,
        canImport: true,
      },
    ],
    ...snapshot,
  };
  playerStore.publish(full);
  return render(<PlayerBar />);
}

describe("PlayerBar (task 11.7) — temporary item badge", () => {
  it("shows the 临时 badge when the current entry is a temporary item", () => {
    renderWithSnapshot();
    expect(screen.getByLabelText("临时播放项")).toBeInTheDocument();
    expect(screen.getByLabelText("临时播放项").textContent).toBe("临时");
  });

  it("does NOT show the 临时 badge for a library song", () => {
    renderWithSnapshot({
      currentSongId: "song-abc",
      currentCanImport: false,
      currentTitle: null,
      queue: [
        {
          entryId: "entry-2",
          songId: "song-abc",
          title: null,
          isCurrent: true,
          failed: false,
          canImport: false,
        },
      ],
    });
    expect(screen.queryByLabelText("临时播放项")).not.toBeInTheDocument();
  });
});

describe("PlayerBar (task 11.7) — import to library button", () => {
  it("shows the import button when a temporary item is playing", () => {
    renderWithSnapshot();
    expect(screen.getByRole("button", { name: "导入到资料库" })).toBeInTheDocument();
  });

  it("does NOT show the import button for a library song", () => {
    renderWithSnapshot({
      currentSongId: "song-abc",
      currentCanImport: false,
      queue: [
        {
          entryId: "entry-2",
          songId: "song-abc",
          title: null,
          isCurrent: true,
          failed: false,
          canImport: false,
        },
      ],
    });
    expect(screen.queryByRole("button", { name: "导入到资料库" })).not.toBeInTheDocument();
  });

  it("sends import_current_temporary_file when clicked", async () => {
    renderWithSnapshot();
    const btn = screen.getByRole("button", { name: "导入到资料库" });
    // The mock returns a success result for import_current_temporary_file.
    capturedInvoke.mockResolvedValueOnce({ kind: "imported", songId: "new-123" });
    fireEvent.click(btn);
    // Wait for the async handler.
    await screen.findByTestId("playerbar");
    expect(capturedInvoke).toHaveBeenCalledWith("import_current_temporary_file", {});
  });

  it("shows importing state while the command is in flight", async () => {
    renderWithSnapshot();
    // Return a never-resolving promise to simulate a slow import.
    capturedInvoke.mockReturnValueOnce(new Promise(() => {}));
    fireEvent.click(screen.getByRole("button", { name: "导入到资料库" }));
    expect(screen.getByText("导入中…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "导入到资料库" })).toBeDisabled();
  });

  it("shows result text after import completes", async () => {
    renderWithSnapshot();
    capturedInvoke.mockResolvedValueOnce({ kind: "imported" });
    fireEvent.click(screen.getByRole("button", { name: "导入到资料库" }));
    await screen.findByText("已导入到资料库");
    expect(screen.getByText("已导入到资料库")).toBeInTheDocument();
  });
});

describe("PlayerBar — empty state", () => {
  it("shows 未在播放 when nothing is current", () => {
    render(<PlayerBar />);
    expect(screen.getByText("未在播放")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "导入到资料库" })).not.toBeInTheDocument();
  });
});

describe("PlayerBar (task 11.8) — range keyboard stepping", () => {
  it("seek range steps by 5 seconds", () => {
    renderWithSnapshot();
    const seek = screen.getByRole("slider", { name: "播放进度" });
    expect(seek).toHaveAttribute("step", "5");
    expect(seek).toHaveAttribute("min", "0");
    expect(seek).toHaveAttribute("max", "120");
  });

  it("volume range steps by 5%", () => {
    renderWithSnapshot();
    const volume = screen.getByRole("slider", { name: "音量" });
    expect(volume).toHaveAttribute("step", "0.05");
  });

  it("seek exposes a descriptive aria-valuetext (time based)", () => {
    renderWithSnapshot();
    const seek = screen.getByRole("slider", { name: "播放进度" });
    // position 30 / duration 120 → "0:30 / 2:00"
    expect(seek.getAttribute("aria-valuetext")).toMatch(/0:30/);
  });

  it("announces media status via a live region", () => {
    renderWithSnapshot();
    const status = screen.getByTestId("player-status-text");
    expect(status.textContent).toContain("正在播放");
    expect(status.getAttribute("aria-live")).toBe("polite");
  });
});
