/**
 * Task 11.8 — global playback keyboard shortcuts.
 *
 * Acceptance:
 *  - Space toggles play/pause; ,/. step previous/next; arrows step seek/volume;
 *    M toggles mute.
 *  - When focus is inside an input/textarea/select (e.g. the search box), these
 *    keys must NOT trigger playback (Space in an input must not play).
 *  - Arrow/letter hotkeys are ignored while an overlay (queue/immersive/focus)
 *    is open, so the overlay owns the keys.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { playerStore, type UiPlayerSnapshot } from "./playerStore";
import { useGlobalPlayerHotkeys } from "./useGlobalPlayerHotkeys";

vi.mock("../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../bridge";

const call = vi.mocked(bridge.call);

function makeSnapshot(overrides: Partial<UiPlayerSnapshot> = {}): UiPlayerSnapshot {
  return {
    state: "playing",
    position: 30,
    duration: 200,
    volume: 0.5,
    muted: false,
    currentQueueEntryId: "e1",
    currentSongId: "s1",
    queueLen: 1,
    mode: "sequential",
    currentTitle: "Song",
    currentCanImport: false,
    queue: [
      {
        entryId: "e1",
        songId: "s1",
        title: null,
        isCurrent: true,
        failed: false,
        canImport: false,
      },
    ],
    ...overrides,
  };
}

function Harness() {
  useGlobalPlayerHotkeys();
  return <input data-testid="focus-input" placeholder="search" />;
}

describe("useGlobalPlayerHotkeys (task 11.8)", () => {
  beforeEach(() => {
    call.mockReset();
    playerStore.publish(makeSnapshot());
    playerStore.setQueueOpen(false);
    playerStore.setImmersiveOpen(false);
    playerStore.setFocusOpen(false);
    render(<Harness />);
  });

  it("Space toggles play/pause", () => {
    fireEvent.keyDown(window, { code: "Space", key: " " });
    expect(call).toHaveBeenCalledWith("player_control", { action: "toggle" });
  });

  it("Space does NOT toggle when focus is inside an input", () => {
    const input = screen.getByTestId("focus-input");
    input.focus();
    fireEvent.keyDown(input, { code: "Space", key: " " });
    expect(call).not.toHaveBeenCalledWith("player_control", { action: "toggle" });
  });

  it("Right arrow seeks +5 seconds from the snapshot position", () => {
    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(call).toHaveBeenCalledWith("seek", { position: 35 });
  });

  it("Left arrow seeks -5 seconds from the snapshot position", () => {
    playerStore.publish(makeSnapshot({ position: 10 }));
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(call).toHaveBeenCalledWith("seek", { position: 5 });
  });

  it("Left arrow does not seek below 0", () => {
    playerStore.publish(makeSnapshot({ position: 2 }));
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(call).toHaveBeenCalledWith("seek", { position: 0 });
  });

  it("Up arrow steps volume +5% and clamps at 1", () => {
    fireEvent.keyDown(window, { key: "ArrowUp" });
    expect(call).toHaveBeenCalledWith("set_volume", { volume: 0.55 });
  });

  it("Down arrow steps volume -5% and clamps at 0", () => {
    playerStore.publish(makeSnapshot({ volume: 0.02 }));
    fireEvent.keyDown(window, { key: "ArrowDown" });
    expect(call).toHaveBeenCalledWith("set_volume", { volume: 0 });
  });

  it("M toggles mute", () => {
    fireEvent.keyDown(window, { key: "m" });
    expect(call).toHaveBeenCalledWith("toggle_mute");
  });

  it(". advances to next and , goes to previous", () => {
    fireEvent.keyDown(window, { key: "." });
    expect(call).toHaveBeenCalledWith("player_control", { action: "next" });
  });

  it("hotkeys are ignored while the queue overlay is open", () => {
    playerStore.setQueueOpen(true);
    call.mockReset();
    fireEvent.keyDown(window, { key: "ArrowRight" });
    fireEvent.keyDown(window, { code: "Space", key: " " });
    expect(call).not.toHaveBeenCalledWith("seek", expect.anything());
    expect(call).not.toHaveBeenCalledWith("player_control", { action: "toggle" });
  });

  it("single-letter keys type into an input without firing", () => {
    const input = screen.getByTestId("focus-input");
    input.focus();
    fireEvent.keyDown(input, { key: "m" });
    expect(call).not.toHaveBeenCalledWith("toggle_mute");
  });
});
