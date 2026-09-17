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

import { render, screen, fireEvent, waitFor } from "@testing-library/react";
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
        blocked: false,
        artist: null,
        durationS: null,
        coverKey: null,
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
          blocked: false,
          artist: null,
          durationS: null,
          coverKey: null,
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
          blocked: false,
          artist: null,
          durationS: null,
          coverKey: null,
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

describe("PlayerBar — empty queue", () => {
  it("never prints 未在播放 — the four regions still draw", () => {
    render(<PlayerBar />);
    expect(screen.queryByText("未在播放")).not.toBeInTheDocument();
    expect(screen.getByTestId("playerbar")).toHaveAttribute("data-empty", "true");
    // 当前播放区: the empty song slot, not a sentence.
    expect(screen.getByTestId("now-playing-trigger")).toBeInTheDocument();
    // 传输控制区 / 音频与队列区 are still there.
    expect(screen.getByRole("button", { name: "上一首" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "下一首" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "列表循环" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "显示播放队列" })).toBeInTheDocument();
    expect(screen.getByRole("slider", { name: "音量" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "导入到资料库" })).not.toBeInTheDocument();
  });

  it("disables track-specific transport but keeps global controls live", () => {
    render(<PlayerBar />);
    expect(screen.getByRole("button", { name: "上一首" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "播放" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下一首" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "喜欢当前歌曲" })).toBeDisabled();
    expect(screen.getByRole("slider", { name: "播放进度" })).toBeDisabled();
    // These controls do not need a current track. Mode remains selectable so
    // the next queued song starts with the user's chosen playback behavior.
    expect(screen.getByRole("button", { name: "列表循环" })).toBeEnabled();
    capturedInvoke.mockResolvedValueOnce(null);
    fireEvent.click(screen.getByRole("button", { name: "列表循环" }));
    expect(capturedInvoke).toHaveBeenCalledWith("player_control", { action: "mode:shuffle" });
    // Neither of these depends on the current track either.
    expect(screen.getByRole("slider", { name: "音量" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "显示播放队列" })).toBeEnabled();
  });

  it("has no data-empty flag while a track is current", () => {
    // The default snapshot is mid-playback, so the transport reads 暂停.
    renderWithSnapshot();
    expect(screen.getByTestId("playerbar")).not.toHaveAttribute("data-empty");
    expect(screen.getByRole("button", { name: "暂停" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "上一首" })).toBeEnabled();
  });

  it("disables seeking until the current track has a usable duration", () => {
    renderWithSnapshot({ state: "loading", duration: null, position: null });
    expect(screen.getByRole("slider", { name: "播放进度" })).toBeDisabled();
  });
});

describe("PlayerBar — 内置封面 (design §115 内置优先)", () => {
  /** renderWithSnapshot with a real library song id. */
  function renderLibrarySong() {
    return renderWithSnapshot({
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
          blocked: false,
          artist: null,
          durationS: null,
          coverKey: null,
        },
      ],
    });
  }

  it("renders the embedded artwork when the backend resolves a key", async () => {
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("song_cover_keys", { "song-abc": "cv1-abc" });
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("song_detail", {
      songId: "song-abc",
      relativePath: "a.mp3",
      title: "心房",
      artist: "陈婧霏",
      album: null,
      durationS: 120,
      format: "mp3",
      playCount: 0,
      favorite: false,
      hasCover: true,
      availability: "available",
    });
    const { container } = renderLibrarySong();
    const img = await waitFor(() => {
      const found = container.querySelector<HTMLImageElement>(".mini-cover img");
      if (!found) throw new Error("artwork not rendered yet");
      return found;
    });
    expect(img.getAttribute("src")).toBe("cover://cv1-abc");
    expect(container.querySelector(".mini-cover")).toHaveClass("has-image");
    // The 当前播放区 shows the real title/artist once `song_detail` answers.
    expect(screen.getByText("心房")).toBeInTheDocument();
    expect(screen.getByText("陈婧霏")).toBeInTheDocument();
  });

  it("keeps the vinyl placeholder for a song with no embedded artwork", async () => {
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("song_cover_keys", {});
    const { container } = renderLibrarySong();
    await waitFor(() =>
      expect(capturedInvoke).toHaveBeenCalledWith("song_cover_keys", { songIds: ["song-abc"] }),
    );
    expect(container.querySelector(".mini-cover img")).toBeNull();
    expect(container.querySelector(".mini-cover")).not.toHaveClass("has-image");
  });
});

describe("PlayerBar (task 11.8) — range keyboard stepping", () => {
  beforeEach(() => {
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("seek", null);
  });

  it("keeps fractional playback positions instead of snapping the thumb to 5 seconds", () => {
    renderWithSnapshot({ state: "paused", position: 32.125 });
    const seek = screen.getByRole("slider", { name: "播放进度" });
    expect(seek).toHaveAttribute("step", "any");
    expect(seek).toHaveValue("32.125");
    expect(seek).toHaveAttribute("min", "0");
    expect(seek).toHaveAttribute("max", "120");
  });

  it("uses the library duration so a paused restored track is seekable immediately", async () => {
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("song_detail", {
      songId: "song-restored",
      relativePath: "restored.flac",
      title: "恢复的歌曲",
      artist: "Echo",
      album: null,
      durationS: 245,
      playCount: 0,
      favorite: false,
      hasCover: false,
      availability: "available",
    });
    renderWithSnapshot({
      state: "paused",
      position: 37.5,
      duration: null,
      currentSongId: "song-restored",
      currentCanImport: false,
      queue: [
        {
          entryId: "entry-restored",
          songId: "song-restored",
          title: "恢复的歌曲",
          isCurrent: true,
          failed: false,
          canImport: false,
          blocked: false,
          artist: "Echo",
          durationS: 245,
          coverKey: null,
        },
      ],
    });

    await waitFor(() => expect(screen.getByRole("slider", { name: "播放进度" })).toBeEnabled());
    const seek = screen.getByRole("slider", { name: "播放进度" });
    expect(seek).toHaveValue("37.5");
    expect(seek).toHaveAttribute("max", "245");
  });

  it.each([
    ["ArrowRight", 30, 35],
    ["ArrowUp", 30, 35],
    ["ArrowLeft", 30, 25],
    ["ArrowDown", 30, 25],
    ["ArrowLeft", 2, 0],
    ["ArrowRight", 118, 120],
    ["Home", 30, 0],
    ["End", 30, 120],
  ])("seeks with %s from %s to %s seconds", (key, position, expected) => {
    renderWithSnapshot({ state: "paused", position });
    fireEvent.keyDown(screen.getByRole("slider", { name: "播放进度" }), { key });
    expect(capturedInvoke).toHaveBeenCalledWith("seek", { position: expected });
  });

  it("allows dragging to a fractional second", () => {
    renderWithSnapshot({ state: "paused" });
    fireEvent.change(screen.getByRole("slider", { name: "播放进度" }), {
      target: { value: "32.125" },
    });
    expect(capturedInvoke).toHaveBeenCalledWith("seek", { position: 32.125 });
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
