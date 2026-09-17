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
 *        and no source label — the lyrics own the whole block.
 * 11.6: lyrics focus mode is the prototype's `body.lyrics-focus-open` — the
 *        same surface rearranged (cover/meta hidden, lyrics take the whole
 *        area) with the 常驻播放栏 carrying the minimal controls; Escape and
 *        the 收起 button exit focus without closing the player.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { playerStore, type UiPlayerSnapshot } from "../../player/playerStore";
import { ImmersivePlayer } from "./ImmersivePlayer";

vi.mock("../../bridge", () => {
  const call = vi.fn();
  return {
    bridge: {
      call,
      // Task 6.1 keeps the fire-and-forget path observable: delegate to the
      // same spy so assertions about "which command was sent" still hold.
      fireAndForget: (command: string, ...args: unknown[]) => {
        void call(command, ...args);
      },
    },
    // The real shell composes `cover://<key>`; the test only needs a URL that
    // carries the opaque key.
    assetUrl: (key: string) => `cover://${key}`,
  };
});

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

/** Mock song_detail + get_lyrics + song_cover_keys; lyrics defaults to empty. */
function mockService(
  meta: { title?: string; hasCover?: boolean; coverKeys?: Record<string, string> } = {},
  lyrics?: {
    source?: "override" | "embedded" | "sidecar" | null;
    timed?: boolean;
    lines?: { seconds: number; text: string }[];
    plainText?: string;
    parseError?: string | null;
  },
) {
  call.mockReset();
  const lyricsData = {
    source: null as "override" | "embedded" | "sidecar" | null,
    timed: false,
    lines: [] as { seconds: number; text: string }[],
    plainText: "",
    parseError: null as string | null,
    ...lyrics,
  };
  call.mockImplementation((async (cmd: string, ...rest: unknown[]) => {
    if (cmd === "song_detail")
      return {
        songId: (rest[0] as Record<string, unknown>)?.songId,
        title: meta.title ?? "Song",
        hasCover: meta.hasCover ?? true,
      };
    if (cmd === "get_lyrics") return lyricsData;
    // 内置封面 (design §115 内置优先): a song without embedded artwork is simply
    // absent from the map.
    if (cmd === "song_cover_keys") return meta.coverKeys ?? {};
    return undefined;
  }) as never);
}

describe("ImmersivePlayer (tasks 11.3–11.6)", () => {
  // The player store is a module-level external store shared by every test, so
  // the overlay/focus flags have to be reset or one test's focus state leaks
  // into the next one's DOM. 歌词专注阅读 also drives `body`, which outlives the
  // component under test, so it is cleared here too.
  beforeEach(() => {
    playerStore.setImmersiveOpen(false);
    playerStore.setFocusOpen(false);
    document.body.classList.remove("player-mode-open", "lyrics-focus-open");
  });

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
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    const { container } = render(<ImmersivePlayer />);
    expect(container.querySelector(".vinyl-nocover")).not.toBeNull();
    expect(await screen.findByText("No Art")).toBeInTheDocument();
  });

  it("puts the embedded artwork on the record label (内置优先)", async () => {
    mockService({ title: "Art Song", hasCover: true, coverKeys: { s1: "cv1-abc" } });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    const { container } = render(<ImmersivePlayer />);
    const img = await waitFor(() => {
      const found = container.querySelector<HTMLImageElement>(".ring-art");
      if (!found) throw new Error("artwork not rendered yet");
      return found;
    });
    expect(img.getAttribute("src")).toBe("cover://cv1-abc");
    // A resolved artwork means the drawn label placeholder is not in play.
    expect(container.querySelector(".vinyl-nocover")).toBeNull();
  });

  it("collapsing does not stop playback", async () => {
    mockService();
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
    call.mockImplementation((async (cmd: string, ...rest: unknown[]) => {
      const songId = (rest[0] as Record<string, unknown>)?.songId;
      if (cmd === "song_detail")
        return { songId, title: songId === "s1" ? "One" : "Two", hasCover: true };
      if (cmd === "get_lyrics")
        return { source: null, timed: false, lines: [], plainText: "", parseError: null };
      return undefined;
    }) as never);

    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
            title: null,
            isCurrent: true,
            failed: false,
            canImport: false,
            blocked: false,
            artist: null,
            durationS: null,
            coverKey: null,
          },
          {
            entryId: "e2",
            songId: "s2",
            title: null,
            isCurrent: false,
            failed: false,
            canImport: false,
            blocked: false,
            artist: null,
            durationS: null,
            coverKey: null,
          },
        ],
      }),
    );
    const { rerender } = render(<ImmersivePlayer />);
    expect(await screen.findByText("One")).toBeInTheDocument();
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e2",
        currentSongId: "s2",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
            title: null,
            isCurrent: false,
            failed: false,
            canImport: false,
            blocked: false,
            artist: null,
            durationS: null,
            coverKey: null,
          },
          {
            entryId: "e2",
            songId: "s2",
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
      }),
    );
    rerender(<ImmersivePlayer />);
    expect(await screen.findByText("Two")).toBeInTheDocument();
  });

  // ---- 11.4: synced lyrics ----

  it("renders synced lyrics with the current line from the authoritative position", async () => {
    mockService(
      {},
      {
        source: "embedded",
        timed: true,
        lines: [
          { seconds: 0, text: "A" },
          { seconds: 5, text: "B" },
          { seconds: 10, text: "C" },
        ],
      },
    );
    playerStore.setImmersiveOpen(true);
    // position=7 → B is current (5 <= 7 < 10).
    playerStore.publish(
      makeSnapshot({
        position: 7,
        duration: 30,
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    const list = await screen.findByTestId("lyrics-list");
    const lines = list.querySelectorAll(".lyric-line");
    expect(lines).toHaveLength(3);
    expect(lines[1]).toHaveClass("is-current");
    expect(lines[1]).toHaveAttribute("aria-current", "true");
    expect(lines[2]).not.toHaveClass("is-current");
  });

  it("clicking a synced line sends seek to that line's timestamp", async () => {
    mockService(
      {},
      {
        source: "sidecar",
        timed: true,
        lines: [
          { seconds: 0, text: "First" },
          { seconds: 12, text: "Second" },
        ],
      },
    );
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    const list = await screen.findByTestId("lyrics-list");
    const before = call.mock.calls.filter(([c]) => c === "seek").length;
    fireEvent.click(list.querySelectorAll(".lyric-line")[1]);
    const seekCalls = call.mock.calls.filter(([c]) => c === "seek");
    expect(seekCalls.length).toBe(before + 1);
    expect(seekCalls[seekCalls.length - 1][1]).toEqual({ position: 12 });
  });

  it("carries no header row above the lyrics", async () => {
    mockService({}, { source: "sidecar", timed: true, lines: [{ seconds: 0, text: "Hi" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    await screen.findByTestId("lyrics-list");

    // The lyrics own the whole block. No source label (the product dropped it),
    // no reading-toggle button either — the entry is the lyrics area itself —
    // and 回到当前行 exists only while auto-follow is paused, so a freshly
    // opened surface shows no row above the column at all.
    expect(document.querySelector(".lyrics-label")).toBeNull();
    expect(document.querySelector(".lyrics-actions")).toBeNull();
    expect(screen.queryByText(/歌词来源/)).toBeNull();
    expect(screen.queryByText("歌词专注阅读")).toBeNull();
    expect(screen.queryByText("回到当前行")).toBeNull();
  });

  // ---- 11.5: plain text / no lyrics ----

  it("renders plain text lyrics as a static list", async () => {
    mockService(
      {},
      {
        source: "embedded",
        timed: false,
        plainText: "Line one\nLine two",
      },
    );
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    const plainList = await screen.findByTestId("lyrics-plain-list");
    const plainLines = plainList.querySelectorAll(".lyrics-line-plain");
    expect(plainLines).toHaveLength(2);
    expect(plainLines[0].textContent).toBe("Line one");
  });

  it("shows the no-lyrics empty state and clears it on track change", async () => {
    mockService();
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    const { rerender } = render(<ImmersivePlayer />);
    expect(await screen.findByTestId("lyrics-empty")).toBeInTheDocument();
    expect(screen.getByText("暂无歌词")).toBeInTheDocument();

    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e2",
        currentSongId: "s2",
        queue: [
          {
            entryId: "e2",
            songId: "s2",
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
      }),
    );
    rerender(<ImmersivePlayer />);
    expect(await screen.findByTestId("lyrics-empty")).toBeInTheDocument();
  });

  // ---- 11.6: lyrics focus mode ----

  it("enters focus by rearranging the same surface, not by stacking a second one", async () => {
    mockService(
      {},
      { source: "embedded", timed: true, lines: [{ seconds: 0, text: "Focus line" }] },
    );
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    // The entry is the lyrics area itself (the prototype's `lyricsCard`): the
    // 歌词专注阅读 button is gone, activating the block is the whole gesture.
    const block = await screen.findByTestId("lyrics");
    expect(screen.queryByText("歌词专注阅读")).toBeNull();
    fireEvent.click(block);

    // 歌词专注阅读 is the prototype's `body.lyrics-focus-open`: the immersive
    // surface rearranged in place. There is no second surface to find — the
    // body switch and the lyrics block's state carry it.
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(true);
    expect(block).toHaveClass("is-focus");
    // The lyrics still render (so focus is never a blank screen), and the block
    // now announces the way back out.
    expect(await screen.findByText("Focus line")).toBeInTheDocument();
    expect(block).toHaveAccessibleName("退出歌词专注阅读");
  });

  it("enters focus from the keyboard too, without a button of its own", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "K" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    const block = await screen.findByTestId("lyrics");
    expect(block).toHaveAttribute("tabindex", "0");
    expect(block).toHaveAccessibleName("展开歌词专注阅读");

    // Enter and Space are the prototype's own activation keys (1793).
    fireEvent.keyDown(block, { key: "Enter" });
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(true);
    fireEvent.keyDown(block, { key: " " });
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(false);

    // A lyric line keeps its own Enter (seek), it must not also flip reading.
    fireEvent.keyDown(screen.getByText("K"), { key: "Enter" });
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(false);
  });

  it("does not repeat the player bar's transport inside the surface", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "X" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.setFocusOpen(true);
    playerStore.publish(
      makeSnapshot({
        state: "paused",
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    await screen.findAllByText("X");

    // The 常驻播放栏 below carries every transport control, in focus mode and
    // out of it; a set inside the surface would duplicate each button and eat
    // the lyrics column. This harness renders only the surface, so the surface
    // must expose none of them.
    const surface = screen.getByTestId("immersive");
    expect(surface.querySelector(".controls")).toBeNull();
    for (const name of ["播放", "暂停", "上一首", "下一首", "列表循环"]) {
      expect(screen.queryByLabelText(name)).toBeNull();
    }
    expect(document.querySelector(".lyrics-focus-controls")).toBeNull();
  });

  it("Escape exits focus without closing the immersive player", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "X" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.setFocusOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    await screen.findByTestId("lyrics");
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(true);

    fireEvent.keyDown(window, { key: "Escape" });
    // Focus unwinds first (the spec's Escape order) and the immersive surface
    // itself stays open behind it.
    await waitFor(() => expect(document.body.classList.contains("lyrics-focus-open")).toBe(false));
    expect(playerStore.getUi().focusOpen).toBe(false);
    expect(screen.getByTestId("immersive")).toBeInTheDocument();
  });

  it("leaves focus via the 收起 button without closing the immersive player", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "X" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.setFocusOpen(true);
    playerStore.publish(
      makeSnapshot({
        state: "paused",
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    await screen.findAllByText("X");

    // The prototype rewrites this button's label and makes it the way out of
    // 歌词专注阅读 (echo-desktop-player.html:1787/1790).
    const exit = screen.getByTestId("close-player-mode");
    expect(exit).toHaveAccessibleName("退出歌词专注阅读");

    fireEvent.click(exit);
    await waitFor(() => expect(document.body.classList.contains("lyrics-focus-open")).toBe(false));
    expect(screen.getByTestId("immersive")).toBeInTheDocument();
    // …and it reverts to closing the player rather than the focus state.
    expect(screen.getByTestId("close-player-mode")).toHaveAccessibleName("收起播放器");
  });

  it("toggles focus by clicking the lyrics block, and leaves lyric lines to seek", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "Line A" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    const block = await screen.findByTestId("lyrics");

    // The prototype makes the whole lyrics block the toggle (`lyricsCard`).
    fireEvent.click(block);
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(true);
    fireEvent.click(block);
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(false);

    // A lyric line is a seek target, not a toggle: clicking it must not flip
    // the reading state as a side effect.
    fireEvent.click(screen.getByText("Line A"));
    expect(call).toHaveBeenCalledWith("seek", { position: 0 });
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(false);
  });

  it("offers focus for plain-text lyrics too, not only synced ones", async () => {
    mockService({}, { source: "embedded", timed: false, plainText: "plain line" });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);

    // The entry is not gated on `timed` any more: spec only requires entering
    // from the lyrics area, and the whole 歌词专注阅读 button is gone.
    const block = await screen.findByTestId("lyrics");
    fireEvent.click(block);
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(true);
    expect(screen.getByText("plain line")).toBeInTheDocument();
  });

  it("restores auto-scroll via 回到当前行 after manual scroll pauses it", async () => {
    mockService({}, { source: "embedded", timed: true, lines: [{ seconds: 0, text: "A" }] });
    playerStore.setImmersiveOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        currentSongId: "s1",
        queue: [
          {
            entryId: "e1",
            songId: "s1",
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
      }),
    );
    render(<ImmersivePlayer />);
    await screen.findByTestId("lyrics-list");
    // Manual scroll pauses auto-scroll and reveals the "回到当前行" action. The
    // listener sits on the immersive surface (capture phase) because which
    // element scrolls depends on the layout — and it listens to the *input*
    // (wheel), not to the scroll event, because auto-follow scrolls the column
    // itself and the two cannot be told apart downstream.
    fireEvent.wheel(screen.getByTestId("lyrics"));
    const back = await screen.findByText("回到当前行");
    fireEvent.click(back);
    // After clicking back, the action is no longer offered (auto-scroll resumed).
    expect(screen.queryByText("回到当前行")).not.toBeInTheDocument();
    // …and the floating action must not double as the block's reading toggle:
    // the block toggles on activation, so the click has to stop at the button.
    expect(document.body.classList.contains("lyrics-focus-open")).toBe(false);
  });
});
