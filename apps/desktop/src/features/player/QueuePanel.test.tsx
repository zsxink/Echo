/**
 * Task 11.2 — playback queue panel.
 *
 * Acceptance: renders the authoritative queue projection (current + pending,
 * with failed entries surfaced); "清空待播" removes only pending entries (never
 * stops the current song, never touches the library or playlists); the empty
 * state offers a browse-library action. The panel never fabricates queue data —
 * it renders exactly the published projection.
 *
 * Issue #112 — the projection arrives on its own event, and the list renders
 * only the rows around the viewport, so a long queue costs a handful of DOM
 * rows instead of one per entry.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  playerStore,
  usePlayerUi,
  type UiPlayerSnapshot,
  type UiQueueProjection,
} from "../../player/playerStore";
import { QueuePanel } from "./QueuePanel";

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
    assetUrl: (key: string, size = "list") => `cover://${key}?s=${size}`,
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
    queueRevision: 0,
    ...overrides,
  };
}

function makeEntry(
  overrides: Partial<UiQueueProjection["entries"][number]> = {},
): UiQueueProjection["entries"][number] {
  return {
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
    ...overrides,
  };
}

/** Publish a queue projection the way the desktop's `player://queue` does. */
function publishQueue(entries: UiQueueProjection["entries"], revision = 1): void {
  playerStore.publishQueue({ revision, entries });
}

// The player store is a module-level singleton, so its queue slot survives
// between the cases in this file. A projection that is not newer than the
// applied one is ignored (DP-R22-S02), so each case starts from an empty
// queue at revision 0.
beforeEach(() => {
  playerStore.stateForRender().update((state) => ({
    ...state,
    queue: { revision: 0, entries: [] },
  }));
});

describe("QueuePanel (task 11.2)", () => {
  it("renders nothing when the panel is closed", () => {
    playerStore.setQueueOpen(false);
    render(<QueuePanel />);
    expect(screen.queryByTestId("queue-panel")).not.toBeInTheDocument();
  });

  it("renders current and pending entries from the authoritative projection", () => {
    playerStore.setQueueOpen(true);
    playerStore.publish(makeSnapshot({ currentQueueEntryId: "e1", queueLen: 3 }));
    publishQueue([
      makeEntry({
        entryId: "e1",
        songId: "s1",
        title: "晴天",
        isCurrent: true,
        artist: "周杰伦",
        durationS: 239,
        coverKey: "cover-s1",
      }),
      makeEntry({
        entryId: "e2",
        songId: "s2",
        title: "七里香",
        artist: "周杰伦",
        durationS: 245,
      }),
      makeEntry({
        entryId: "e3",
        songId: null,
        title: "outside.m4a",
        canImport: true,
      }),
    ]);

    render(<QueuePanel />);
    expect(screen.getByTestId("queue-list")).toBeInTheDocument();
    expect(screen.getByText("晴天")).toBeInTheDocument();
    expect(screen.getAllByText("周杰伦").length).toBe(2);
    expect(screen.getByText("3:59")).toBeInTheDocument();
    expect(screen.getByTestId("queue-cover-image")).toHaveAttribute(
      "src",
      "cover://cover-s1?s=list",
    );
    expect(screen.getByText("outside.m4a")).toBeInTheDocument();
  });

  it("flags failed entries but still renders them", () => {
    playerStore.setQueueOpen(true);
    publishQueue([
      makeEntry({ entryId: "e1", isCurrent: true }),
      makeEntry({ entryId: "e2", failed: true }),
    ]);

    render(<QueuePanel />);
    expect(screen.getByText("加载失败")).toBeInTheDocument();
  });

  it("plays a selectable queue entry by its queue-entry identity", () => {
    call.mockReset();
    call.mockResolvedValue(undefined);
    playerStore.setQueueOpen(true);
    publishQueue([
      makeEntry({
        entryId: "current-copy",
        songId: "same-song",
        title: "重复曲目",
        isCurrent: true,
        artist: "Echo",
        durationS: 120,
      }),
      makeEntry({
        entryId: "selected-copy",
        songId: "same-song",
        title: "重复曲目",
        artist: "Echo",
        durationS: 120,
      }),
    ]);

    render(<QueuePanel />);
    fireEvent.click(screen.getAllByRole("button", { name: "播放 重复曲目" })[1]);
    expect(call).toHaveBeenCalledWith("queue_command", {
      command: "playEntry",
      entryId: "selected-copy",
    });
  });

  it("keeps blocked restored entries visible with retry guidance", () => {
    playerStore.setQueueOpen(true);
    publishQueue([makeEntry({ entryId: "blocked", blocked: true })]);
    render(<QueuePanel />);
    expect(screen.getByText("暂时不可用，可在音乐库恢复后重试")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "播放 未知歌曲" })).toBeDisabled();
  });

  it("清空待播 sends clearPending and does not stop the current song or touch playlists", () => {
    call.mockReset();
    call.mockResolvedValue(undefined);
    playerStore.setQueueOpen(true);
    publishQueue([
      makeEntry({ entryId: "e1", isCurrent: true }),
      makeEntry({ entryId: "e2" }),
    ]);

    render(<QueuePanel />);
    fireEvent.click(screen.getByText("清空待播"));
    expect(call).toHaveBeenCalledWith("queue_command", { command: "clearPending" });
    // The clear command only targets pending items — it must not stop the
    // current song (no `player_control` pause/stop) nor mutate playlists.
    const queueCalls = call.mock.calls.filter(([cmd]) => cmd === "queue_command");
    expect(queueCalls).toHaveLength(1);
    expect(queueCalls[0][1]).toEqual({ command: "clearPending" });
    expect(call).not.toHaveBeenCalledWith("player_control", expect.anything());
  });

  it("shows the browse-library action for an empty queue", () => {
    playerStore.setQueueOpen(true);
    publishQueue([]);

    render(<QueuePanel />);
    expect(screen.getByTestId("queue-empty")).toBeInTheDocument();
    expect(screen.getByText("浏览曲库")).toBeInTheDocument();
  });
});

describe("QueuePanel windowed rendering (issue #112)", () => {
  it("renders a bounded number of rows for a long queue", () => {
    playerStore.setQueueOpen(true);
    publishQueue(
      Array.from({ length: 500 }, (_, index) =>
        makeEntry({ entryId: `e${index}`, songId: `s${index}`, title: `song ${index}` }),
      ),
    );

    render(<QueuePanel />);
    const rows = screen.getAllByRole("button", { name: /^播放 / });
    expect(rows.length).toBeLessThan(50);
    expect(rows.length).toBeGreaterThan(0);
    // The total count still describes the whole queue (DP-R03).
    expect(screen.getByText("500 首")).toBeInTheDocument();
  });

  it("holds the space of the rows it does not render", () => {
    playerStore.setQueueOpen(true);
    publishQueue(
      Array.from({ length: 500 }, (_, index) =>
        makeEntry({ entryId: `e${index}`, songId: `s${index}`, title: `song ${index}` }),
      ),
    );

    render(<QueuePanel />);
    const window = document.querySelector<HTMLElement>(".queue-window");
    expect(window).not.toBeNull();
    // The spacer below the rendered rows stands for the rest of the queue.
    expect(Number.parseFloat(window?.style.paddingBottom ?? "0")).toBeGreaterThan(0);
  });

  it("moves the window with the scroll offset", () => {
    playerStore.setQueueOpen(true);
    publishQueue(
      Array.from({ length: 500 }, (_, index) =>
        makeEntry({ entryId: `e${index}`, songId: `s${index}`, title: `song ${index}` }),
      ),
    );

    render(<QueuePanel />);
    expect(screen.queryByText("song 300")).not.toBeInTheDocument();

    // jsdom has no layout, so the scroll offset is installed explicitly.
    const list = screen.getByTestId("queue-list");
    Object.defineProperty(list, "scrollTop", {
      configurable: true,
      value: 300 * 44,
    });
    fireEvent.scroll(list);

    expect(screen.getByText("song 300")).toBeInTheDocument();
    // The rows above the viewport are still only a spacer, not DOM rows.
    expect(screen.queryByText("song 0")).not.toBeInTheDocument();
  });

  it("does not rebuild the queue rows while the position stream runs", () => {
    playerStore.setQueueOpen(true);
    publishQueue(
      Array.from({ length: 20 }, (_, index) =>
        makeEntry({ entryId: `e${index}`, songId: `s${index}`, title: `song ${index}` }),
      ),
    );

    render(<QueuePanel />);
    const list = screen.getByTestId("queue-list");
    const before = list.querySelectorAll(".queue-item").length;

    for (let frame = 0; frame < 10; frame += 1) {
      playerStore.publish(
        makeSnapshot({ state: "playing", position: frame, queueLen: 20, queueRevision: 1 }),
      );
    }

    expect(list.querySelectorAll(".queue-item").length).toBe(before);
    expect(screen.getByText("song 0")).toBeInTheDocument();
  });
});

/**
 * fix-queue-trigger-toggle — the trigger lives in the player bar, a sibling of the
 * popover, so the panel only reaches it through an explicit `triggerRef`.
 */
describe("QueuePanel trigger control (fix-queue-trigger-toggle)", () => {
  /** The popover and its trigger side by side, as `App` renders them. */
  function QueueWithTrigger() {
    const triggerRef = useRef<HTMLButtonElement>(null);
    const queueOpen = usePlayerUi().queueOpen;
    return (
      <div>
        <button
          type="button"
          ref={triggerRef}
          data-testid="queue-trigger"
          aria-expanded={queueOpen}
          onClick={() => playerStore.setQueueOpen(!queueOpen)}
        >
          播放队列
        </button>
        <QueuePanel triggerRef={triggerRef} />
      </div>
    );
  }

  it("closes on a second press of the trigger instead of reopening the panel", () => {
    playerStore.setQueueOpen(false);
    playerStore.publish(makeSnapshot({ queueLen: 1 }));
    render(<QueueWithTrigger />);
    const trigger = screen.getByTestId("queue-trigger");

    fireEvent.click(trigger);
    expect(screen.getByTestId("queue-panel")).toBeInTheDocument();

    // The real gesture on the trigger: pointerdown, then click. Before the fix
    // the press dismissed the panel and the click re-opened it, so the panel
    // never went away and the button looked inert.
    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    expect(screen.queryByTestId("queue-panel")).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("still closes when the press lands outside both panel and trigger", () => {
    playerStore.setQueueOpen(false);
    playerStore.publish(makeSnapshot({ queueLen: 1 }));
    render(
      <div>
        <QueueWithTrigger />
        <button type="button" data-testid="elsewhere">
          其它区域
        </button>
      </div>,
    );
    fireEvent.click(screen.getByTestId("queue-trigger"));
    expect(screen.getByTestId("queue-panel")).toBeInTheDocument();

    fireEvent.pointerDown(screen.getByTestId("elsewhere"));
    expect(screen.queryByTestId("queue-panel")).not.toBeInTheDocument();
  });
});
