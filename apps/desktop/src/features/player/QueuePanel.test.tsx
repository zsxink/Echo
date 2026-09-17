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
  assetUrl: (key: string) => `cover://${key}`,
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
          {
            entryId: "e1",
            songId: "s1",
            title: "晴天",
            isCurrent: true,
            failed: false,
            canImport: false,
            blocked: false,
            artist: "周杰伦",
            durationS: 239,
            coverKey: "cover-s1",
          },
          {
            entryId: "e2",
            songId: "s2",
            title: "七里香",
            isCurrent: false,
            failed: false,
            canImport: false,
            blocked: false,
            artist: "周杰伦",
            durationS: 245,
            coverKey: null,
          },
          {
            entryId: "e3",
            songId: null,
            title: "outside.m4a",
            isCurrent: false,
            failed: false,
            canImport: true,
            blocked: false,
            artist: null,
            durationS: null,
            coverKey: null,
          },
        ],
      }),
    );

    render(<QueuePanel />);
    expect(screen.getByTestId("queue-list")).toBeInTheDocument();
    expect(screen.getByText("晴天")).toBeInTheDocument();
    expect(screen.getAllByText("周杰伦").length).toBe(2);
    expect(screen.getByText("3:59")).toBeInTheDocument();
    expect(screen.getByTestId("queue-cover-image")).toHaveAttribute("src", "cover://cover-s1");
    expect(screen.getByText("outside.m4a")).toBeInTheDocument();
  });

  it("flags failed entries but still renders them", () => {
    playerStore.setQueueOpen(true);
    playerStore.publish(
      makeSnapshot({
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
            failed: true,
            canImport: false,
            blocked: false,
            artist: null,
            durationS: null,
            coverKey: null,
          },
        ],
      }),
    );

    render(<QueuePanel />);
    expect(screen.getByText("加载失败")).toBeInTheDocument();
  });

  it("keeps blocked restored entries visible with retry guidance", () => {
    playerStore.setQueueOpen(true);
    playerStore.publish(
      makeSnapshot({
        queue: [
          {
            entryId: "blocked",
            songId: "s1",
            title: null,
            isCurrent: false,
            failed: false,
            blocked: true,
            canImport: false,
            artist: null,
            durationS: null,
            coverKey: null,
          },
        ],
      }),
    );
    render(<QueuePanel />);
    expect(screen.getByText("暂时不可用，可在资料库恢复后重试")).toBeInTheDocument();
  });

  it("清空待播 sends clearPending and does not stop the current song or touch playlists", () => {
    call.mockReset();
    call.mockResolvedValue(undefined);
    playerStore.setQueueOpen(true);
    playerStore.publish(
      makeSnapshot({
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
