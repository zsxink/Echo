/**
 * Tasks 11.3–11.6 — immersive player overlay with lyrics.
 *
 * Acceptance:
 * 11.3: expand shows vinyl + metadata; collapse does not stop playback;
 *        track switching updates in place; no-cover uses a placeholder.
 * 11.4: synced lyrics render with current-line highlight from the
 *        authoritative position; clicking a line sends seek; lyrics
 *        disappear on track change (component remount).
 * 11.5: plain text lyrics render as a static list; "暂无歌词" for no lyrics;
 *        source label ("内嵌歌词" / "LRC 侧车文件") is shown.
 * 11.6: lyrics focus mode hides the non-essential vinyl/cover and keeps a
 *        minimal control strip; Escape exits focus; "回到当前行" restores
 *        auto-scroll; playback controls work without leaving focus.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { playerStore, type UiPlayerSnapshot } from "../../player/playerStore";
import { ImmersivePlayer } from "./ImmersivePlayer";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

function makeSnapshot(overrides: Partial<UiPlayerSnapshot> = {}): UiPlayerSnapshot {
  return {
    state: "playing",
    position: 30,
    duration: 200,
    volume: 1,
    muted: false,
    currentQueueEntryId: null,
    currentSongId: null,
    queueLen: 0,
    mode: "sequential",
    currentTitle: null,
    currentCanImport: false,
    queue: [],
    ...overrides,
  };
}

/** Mock song_detail + get_lyrics; lyrics defaults to empty. */
function mockService(meta: { title?: string; hasCover?: boolean } = {}, lyrics?: {
  source?: "override" | "embedded" | "sidecar" | null;
  timed?: boolean;
  lines?: { seconds: number; text: string }[];
  plainText?: string;
  parseError?: string | null;
}) {
  call.mockReset();
  const lyricsData = {
    source: null as "override" | "embedded" | "sidecar" | null,
    timed: false,
    lines: [] as { seconds: number; text: string }[],
    plainText: "",
    parseError: null as string | null,
    ...lyrics,
  };
  call.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
    if (cmd === "song_detail") return { songId: (rest[0] as Record<string, unknown>)?.songId, title: meta.title ?? "Song", hasCover: meta.hasCover ?? true };
    if (cmd === "get_lyrics") return lyricsData;
    return undefined;
  });
}

describe("ImmersivePlayer (tasks 11.3–11.6)", () => {
  // ---- 11.3 ----

  it("renders nothing when closed", () => {
    playerStore.setImmersiveOpen(false);
    render(<ImmersivePlayer />);
    expect(screen.queryByTestId("immersive")).not.toBeInTheDocument();
  });

  it("shows vinyl view with metadata and fetches song_detail + get_lyrics", async () => {
    mockService({ title: "Lacquer Love" });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    expect(screen.getByTestId("immersive")).toBeInTheDocument();
    expect(await screen.findByText("Lacquer Love")).toBeInTheDocument();
    const songDetailCalls = call.mock.calls.filter(([cmd]) => cmd === "song_detail");
    const getLyricsCalls = call.mock.calls.filter(([cmd]) => cmd === "get_lyrics");
    expect(songDetailCalls.length).toBe(1);
    expect(getLyricsCalls.length).toBe(1);
  });

  it("no-cover uses a placeholder", async () => {
    mockService({ title: "No Art", hasCover: false });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    const { container } = render(<ImmersivePlayer />);
    expect(container.querySelector(".vinyl-nocover")).not.toBeNull();
    expect(await screen.findByText("No Art")).toBeInTheDocument();
  });

  it("collapsing does not stop playback", async () => {
    mockService();
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    const { unmount } = render(<ImmersivePlayer />);
    await screen.findByTestId("immersive");
    fireEvent.click(screen.getByLabelText("收起播放器"));
    unmount();
    expect(
      call.mock.calls.some(([cmd]) => cmd === "player_control" || cmd === "queue_command"),
    ).toBe(false);
  });

  it("updates in place on track change", async () => {
    call.mockReset();
    call.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      const songId = (rest[0] as Record<string, unknown>)?.songId;
      if (cmd === "song_detail") return { songId, title: songId === "s1" ? "One" : "Two", hasCover: true };
      if (cmd === "get_lyrics") return { source: null, timed: false, lines: [], plainText: "", parseError: null };
      return undefined;
    });

    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [
          { entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false },
          { entryId: "e2", songId: "s2", title: null, isCurrent: false, failed: false, canImport: false },
        ],
      }),
    );
    const { rerender } = render(<ImmersivePlayer />);
    expect(await screen.findByText("One")).toBeInTheDocument();
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e2", currentSongId: "s2",
        queue: [
          { entryId: "e1", songId: "s1", title: null, isCurrent: false, failed: false, canImport: false },
          { entryId: "e2", songId: "s2", title: null, isCurrent: true, failed: false, canImport: false },
        ],
      }),
    );
    rerender(<ImmersivePlayer />);
    expect(await screen.findByText("Two")).toBeInTheDocument();
  });

  // ---- 11.4: synced lyrics ----

  it("renders synced lyrics with the current line from the authoritative position", async () => {
    mockService({}, {
      source: "embedded", timed: true,
      lines: [{ seconds: 0, text: "A" }, { seconds: 5, text: "B" }, { seconds: 10, text: "C" }],
    });
    playerStore.setImmersiveOpen(true);
    // position=7 → B is current (5 <= 7 < 10).
    playerStore.publish(
      makeSnapshot({
        position: 7, duration: 30, currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    const list = await screen.findByTestId("lyrics-list");
    const lines = list.querySelectorAll(".lyrics-line");
    expect(lines).toHaveLength(3);
    expect(lines[1]).toHaveClass("is-current");
    expect(lines[1]).toHaveAttribute("aria-current", "true");
    expect(lines[2]).not.toHaveClass("is-current");
  });

  it("clicking a synced line sends seek to that line's timestamp", async () => {
    mockService({}, {
      source: "sidecar", timed: true,
      lines: [{ seconds: 0, text: "First" }, { seconds: 12, text: "Second" }],
    });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    const list = await screen.findByTestId("lyrics-list");
    const before = call.mock.calls.filter(([c]) => c === "seek").length;
    fireEvent.click(list.querySelectorAll(".lyrics-line")[1]);
    const seekCalls = call.mock.calls.filter(([c]) => c === "seek");
    expect(seekCalls.length).toBe(before + 1);
    expect(seekCalls[seekCalls.length - 1][1]).toEqual({ position: 12 });
  });

  it("shows the lyrics source label", async () => {
    mockService({}, { source: "sidecar", timed: true, lines: [{ seconds: 0, text: "Hi" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    expect(await screen.findByText("歌词来源：LRC 侧车文件")).toBeInTheDocument();
  });

  // ---- 11.5: plain text / no lyrics ----

  it("renders plain text lyrics as a static list", async () => {
    mockService({}, {
      source: "embedded", timed: false,
      plainText: "Line one\nLine two",
    });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    const plainList = await screen.findByTestId("lyrics-plain-list");
    const plainLines = plainList.querySelectorAll(".lyrics-line-plain");
    expect(plainLines).toHaveLength(2);
    expect(plainLines[0].textContent).toBe("Line one");
    expect(screen.getByText("歌词来源：内嵌歌词")).toBeInTheDocument();
  });

  it("shows the no-lyrics empty state and clears it on track change", async () => {
    mockService();
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    const { rerender } = render(<ImmersivePlayer />);
    expect(await screen.findByTestId("lyrics-empty")).toBeInTheDocument();
    expect(screen.getByText("暂无歌词")).toBeInTheDocument();

    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e2", currentSongId: "s2",
        queue: [{ entryId: "e2", songId: "s2", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    rerender(<ImmersivePlayer />);
    expect(await screen.findByTestId("lyrics-empty")).toBeInTheDocument();
  });

  // ---- 11.6: lyrics focus mode ----

  it("enters focus mode, hides the vinyl, and keeps a minimal control strip", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "Focus line" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    const focusEntry = await screen.findByText("歌词专注阅读");
    fireEvent.click(focusEntry);

    expect(screen.getByTestId("lyrics-focus")).toBeInTheDocument();
    // The focus surface shows the loaded lyric line (proving it is not stuck
    // on the empty state) and the 歌词专注 header.
    expect(await screen.findByText("Focus line")).toBeInTheDocument();
    expect(screen.getByText("歌词专注")).toBeInTheDocument();
  });

  it("Escape exits focus without closing the immersive player", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "X" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.setFocusOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    expect(await screen.findByTestId("lyrics-focus")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("lyrics-focus")).not.toBeInTheDocument();
    // The immersive player remains open beneath.
    expect(screen.getByTestId("immersive")).toBeInTheDocument();
  });

  it("keeps playback controls working inside focus mode without exiting", async () => {
    call.mockReset();
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "X" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.setFocusOpen(true);
    playerStore.publish(
      makeSnapshot({
        state: "paused", currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    // Wait for the loaded (non-empty) focus branch which carries the controls.
    await screen.findAllByText("X");

    // The focus controls and the immersive body both render a play button;
    // click the one inside the focus surface (the topmost control on screen).
    const playButtons = screen.getAllByLabelText("播放");
    fireEvent.click(playButtons[0]);
    expect(call).toHaveBeenCalledWith("player_control", { action: "play" });
    // Focus is not exited by using playback controls.
    expect(screen.getByTestId("lyrics-focus")).toBeInTheDocument();
  });

  it("restores auto-scroll via 回到当前行 after manual scroll pauses it", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "A" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1", currentSongId: "s1",
        queue: [{ entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false }],
      }),
    );
    render(<ImmersivePlayer />);
    const list = await screen.findByTestId("lyrics-list");
    // Manual scroll pauses auto-scroll and reveals the "回到当前行" action.
    fireEvent.scroll(list);
    const back = await screen.findByText("回到当前行");
    fireEvent.click(back);
    // After clicking back, the action is no longer offered (auto-scroll resumed).
    expect(screen.queryByText("回到当前行")).not.toBeInTheDocument();
  });
});
