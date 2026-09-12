/**
 * Task 11.2 — playback queue panel.
 *
 * Acceptance: renders the authoritative snapshot queue (current + pending,
 * with failed entries surfaced); "清空待播" removes only pending entries (never
 * stops the current song, never touches the library or playlists); the empty
 * state offers a browse-library action. The panel never fabricates queue data —
 * it renders exactly `snapshot.queue`.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { playerStore, type UiPlayerSnapshot } from "../../player/playerStore";
import { QueuePanel } from "./QueuePanel";

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

describe("QueuePanel (task 11.2)", () => {
  it("renders nothing when the panel is closed", () => {
    playerStore.setQueueOpen(false);
    render(<QueuePanel />);
    expect(screen.queryByTestId("queue-panel")).not.toBeInTheDocument();
  });

  it("renders current and pending entries from the authoritative snapshot", () => {
    playerStore.setQueueOpen(true);
    playerStore.publish(
      makeSnapshot({
        currentQueueEntryId: "e1",
        queueLen: 3,
        queue: [
          { entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false },
          { entryId: "e2", songId: "s2", title: null, isCurrent: false, failed: false, canImport: false },
          { entryId: "e3", songId: null, title: "outside.m4a", isCurrent: false, failed: false, canImport: true },
        ],
      }),
    );

    render(<QueuePanel />);
    expect(screen.getByTestId("queue-list")).toBeInTheDocument();
    // The two library song ids and the temporary title are all rendered.
    expect(screen.getAllByText("资料库歌曲").length).toBe(2);
    expect(screen.getByText("outside.m4a")).toBeInTheDocument();
  });

  it("flags failed entries but still renders them", () => {
    playerStore.setQueueOpen(true);
    playerStore.publish(
      makeSnapshot({
        queue: [
          { entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false },
          { entryId: "e2", songId: "s2", title: null, isCurrent: false, failed: true, canImport: false },
        ],
      }),
    );

    render(<QueuePanel />);
    expect(screen.getByText("加载失败")).toBeInTheDocument();
  });

  it("清空待播 sends clearPending and does not stop the current song or touch playlists", () => {
    call.mockReset();
    call.mockResolvedValue(undefined);
    playerStore.setQueueOpen(true);
    playerStore.publish(
      makeSnapshot({
        queue: [
          { entryId: "e1", songId: "s1", title: null, isCurrent: true, failed: false, canImport: false },
          { entryId: "e2", songId: "s2", title: null, isCurrent: false, failed: false, canImport: false },
        ],
      }),
    );

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
    playerStore.publish(makeSnapshot({ queueLen: 0, queue: [] }));

    render(<QueuePanel />);
    expect(screen.getByTestId("queue-empty")).toBeInTheDocument();
    expect(screen.getByText("浏览曲库")).toBeInTheDocument();
  });
});
