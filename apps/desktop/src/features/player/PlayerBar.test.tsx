/**
 * PlayerBar tests — temporary playback item identification and import (task 11.7).
 *
 * Verifies:
 *  - Temporary items do not carry a visible "临时" badge.
 *  - The persistent player bar exposes a small "导入" button only for
 *    file-browser temporary playback.
 *  - The import button sends the `import_current_temporary_file` command.
 *  - Library songs (with a currentSongId) do NOT show the import button or badge.
 */

import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { PlayerBar } from "./PlayerBar";
import { playerStore } from "../../player/playerStore";
import type { UiPlayerSnapshot } from "../../player/playerStore";
import { invalidateCovers } from "../../app/coverArt";

// Bridge calls are mocked globally by setup.ts; capture them for assertion.
let capturedInvoke: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  // Get a reference to the mocked invoke so we can inspect calls.
  const tauriCore = await import("@tauri-apps/api/core");
  capturedInvoke = (tauriCore as unknown as { invoke: ReturnType<typeof vi.fn> }).invoke;
  capturedInvoke.mockClear();
  invalidateCovers();

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

describe("PlayerBar (task 11.7) — temporary item label", () => {
  it("does not show a 临时 badge when the current entry is temporary", () => {
    renderWithSnapshot();
    expect(screen.queryByLabelText("临时播放项")).not.toBeInTheDocument();
  });

  it("shows a small 导入 button only for a temporary item", () => {
    renderWithSnapshot();
    expect(screen.getByRole("button", { name: "导入" })).toBeInTheDocument();
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
    expect(screen.queryByRole("button", { name: "导入" })).not.toBeInTheDocument();
  });

  it("imports the current temporary file when clicked", () => {
    renderWithSnapshot();
    capturedInvoke.mockResolvedValueOnce(null);
    fireEvent.click(screen.getByRole("button", { name: "导入" }));
    expect(capturedInvoke).toHaveBeenCalledWith("import_current_temporary_file", {});
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
    expect(screen.queryByRole("button", { name: "导入" })).not.toBeInTheDocument();
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

/**
 * DP-R12 非沉浸播放模式视觉语义.
 *
 * The persistent bar must show 随机播放 with a *neutral* style: shuffle may not
 * borrow the theme's accent colour, and switching to it (or switching the theme
 * afterwards) must leave the current song and the queue untouched. Selection is
 * carried by readable semantics (`aria-pressed` / `data-mode`), not by paint.
 *
 * The bar paints the theme accent through exactly one class — `.control.active`
 * (styles/player.css), the 喜欢 heart's selected state — and gives
 * `#playback-mode` no colour of its own. So "the mode button never carries
 * `active`" *is* "shuffle is never painted with the accent", and the first test
 * here pins both halves of that in the same render: the heart has the class, the
 * mode button does not.
 */
describe("PlayerBar — 非沉浸播放模式视觉语义 (DP-R12)", () => {
  const MODE_QUEUE: UiPlayerSnapshot["queue"] = [
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
    {
      entryId: "entry-2",
      songId: null,
      title: "夜曲.flac",
      isCurrent: false,
      failed: false,
      canImport: true,
      blocked: false,
      artist: null,
      durationS: null,
      coverKey: null,
    },
  ];

  /** Publish a playing bar in `mode` without re-mounting (the store is external). */
  function publishMode(mode: UiPlayerSnapshot["mode"], overrides: Partial<UiPlayerSnapshot> = {}) {
    playerStore.publish({
      state: "playing",
      position: 30,
      duration: 120,
      volume: 0.8,
      muted: false,
      currentQueueEntryId: "entry-1",
      currentSongId: null,
      queueLen: 2,
      currentTitle: "晴天.mp3",
      currentCanImport: true,
      mode,
      queue: MODE_QUEUE,
      ...overrides,
    });
  }

  /** The mode control, addressed by the id its own stylesheet targets. */
  function modeButton(): HTMLButtonElement {
    const button = document.querySelector<HTMLButtonElement>("#playback-mode");
    if (!button) throw new Error("#playback-mode is not rendered");
    return button;
  }

  /** Every control action the bar asked the backend to perform. */
  const controlActions = () =>
    capturedInvoke.mock.calls
      .filter(([command]) => command === "player_control")
      .map(([, args]) => (args as { action: string }).action);

  function renderWithTheme(theme: string) {
    return render(
      <div data-echo-theme={theme}>
        <PlayerBar />
      </div>,
    );
  }

  it("keeps the theme accent on the 喜欢 heart, never on the mode button", async () => {
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("song_cover_keys", {});
    // @ts-expect-error - the test hook installed by setup.ts
    globalThis.__echoTest.setInvoke("song_detail", {
      songId: "song-abc",
      relativePath: "a.mp3",
      title: "晴天",
      artist: "Echo Unit",
      album: null,
      durationS: 120,
      format: "mp3",
      playCount: 0,
      favorite: true,
      hasCover: false,
      availability: "available",
    });
    publishMode("shuffle", {
      currentSongId: "song-abc",
      currentTitle: null,
      currentCanImport: false,
    });
    render(<PlayerBar />);

    // The bar paints the theme accent through exactly one selected class —
    // `.control.active` (styles/player.css) — and the 喜欢 heart owns it. This
    // is the control the spec calls 已定义为强调的选中语义.
    const heart = await screen.findByRole("button", { name: "取消喜欢当前歌曲" });
    expect(heart).toHaveClass("active");

    // 随机播放 is just as "on" — it says so readably — but it must not borrow
    // that class, which is what keeps the accent off it under any theme.
    expect(modeButton()).toHaveAttribute("aria-pressed", "true");
    expect(modeButton()).not.toHaveClass("active");
    expect(modeButton().className).toBe("control");
  });

  it("switches to 随机播放 with a neutral button and leaves the queue alone", () => {
    capturedInvoke.mockResolvedValue(null);
    publishMode("sequential");
    render(<PlayerBar />);

    expect(modeButton()).toHaveAttribute("data-mode", "sequential");
    expect(modeButton()).toHaveAttribute("aria-pressed", "false");
    expect(modeButton()).toHaveAccessibleName("列表循环");

    fireEvent.click(modeButton());

    // The switch is a mode change and nothing else — no queue command is issued.
    expect(controlActions()).toEqual(["mode:shuffle"]);
    expect(capturedInvoke).not.toHaveBeenCalledWith("queue_command", expect.anything());

    // The backend confirms 随机播放; the bar adopts it without borrowing paint.
    act(() => publishMode("shuffle"));

    expect(modeButton()).toHaveAttribute("data-mode", "shuffle");
    // 可读语义表明随机播放已选中 …
    expect(modeButton()).toHaveAttribute("aria-pressed", "true");
    expect(modeButton()).toHaveAccessibleName("随机播放");
    // … while the glyph itself stays neutral (no accent-bearing class).
    expect(modeButton().className).toBe("control");
    expect(screen.getByTestId("player-status-text").textContent).toContain("随机播放");
    // The current item is the one the switch started from.
    expect(screen.getByTestId("now-playing-trigger")).toHaveTextContent("晴天.mp3");
  });

  it("keeps 随机播放 neutral while the theme changes", () => {
    publishMode("shuffle");
    const { rerender } = renderWithTheme("coral");

    expect(modeButton()).toHaveAttribute("data-mode", "shuffle");
    expect(modeButton()).toHaveAttribute("aria-pressed", "true");
    const before = modeButton().outerHTML;

    rerender(
      <div data-echo-theme="cobalt">
        <PlayerBar />
      </div>,
    );

    // The control is theme-independent: not a single attribute moved.
    expect(modeButton().outerHTML).toBe(before);
    expect(modeButton()).not.toHaveClass("active");
    expect(modeButton()).toHaveAttribute("data-mode", "shuffle");
    expect(modeButton().className).toBe("control");
    expect(screen.getByTestId("now-playing-trigger")).toHaveTextContent("晴天.mp3");
  });
});
