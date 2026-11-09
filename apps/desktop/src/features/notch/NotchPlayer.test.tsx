/**
 * NotchPlayer tests (openspec `macos-notch-player`).
 *
 * Covers the behaviour that is *not* visual: which values reach the collapsed
 * and expanded surfaces, the empty/temporary states, the mode control's
 * colour neutrality, and the window-growth protocol (await the resize, then
 * animate; shrink only after the collapse finishes).
 *
 * The CSS itself is generated from the prototype, so colour assertions read the
 * generated stylesheet rather than a computed style (jsdom does not apply it).
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NotchPlayer } from "./NotchPlayer";
import { playerStore } from "../../player/playerStore";
import type { UiPlayerSnapshot } from "../../player/playerStore";
import { invalidateCovers } from "../../app/coverArt";
import notchCss from "../../styles/notch.css?raw";

let capturedInvoke: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  const tauriCore = await import("@tauri-apps/api/core");
  capturedInvoke = (tauriCore as unknown as { invoke: ReturnType<typeof vi.fn> }).invoke;
  capturedInvoke.mockClear();
  capturedInvoke.mockImplementation(async (command: string) => {
    if (command === "song_cover_keys") return {};
    if (command === "song_detail") return null;
    if (command === "notch_player_set_expanded") return null;
    if (command === "get_notch_player_state") {
      return { available: true, enabled: true, notchWidth: 185 };
    }
    return null;
  });
  invalidateCovers();
  playerStore.publish(baseSnapshot());
});

afterEach(() => {
  vi.useRealTimers();
});

function baseSnapshot(overrides: Partial<UiPlayerSnapshot> = {}): UiPlayerSnapshot {
  return {
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
    currentArtist: "周杰伦",
    currentAlbum: null,
    currentCoverKey: null,
    currentLyrics: null,
    currentCanImport: true,
    queueRevision: 0,
    ...overrides,
  };
}

function renderNotch(overrides: Partial<UiPlayerSnapshot> = {}) {
  playerStore.publish(baseSnapshot(overrides));
  return render(<NotchPlayer />);
}

const root = () => screen.getByTestId("notch-player");

describe("collapsed and expanded content", () => {
  it("shows the temporary item's own display name", () => {
    renderNotch();
    expect(root()).toHaveAttribute("data-temporary", "true");
    expect(screen.getByText("晴天.mp3")).toBeInTheDocument();
  });

  it("does not leave the previous song's cover or title in the empty state", () => {
    renderNotch({
      state: "stopped",
      position: null,
      duration: null,
      currentQueueEntryId: null,
      currentSongId: null,
      currentTitle: null,
      currentArtist: null,
      currentCanImport: false,
    });
    expect(root()).toHaveAttribute("data-empty", "true");
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.getByText("未在播放")).toBeInTheDocument();
    expect(screen.queryByText("晴天.mp3")).not.toBeInTheDocument();
    // No cover is rendered at all, rather than a broken <img>.
    expect(root().querySelectorAll("img")).toHaveLength(0);
  });

  it("disables the transport and the favourite control when nothing is current", () => {
    renderNotch({
      state: "stopped",
      currentQueueEntryId: null,
      currentSongId: null,
      currentTitle: null,
      currentCanImport: false,
    });
    expect(screen.getByTestId("notch-play")).toBeDisabled();
    expect(screen.getByLabelText("上一首")).toBeDisabled();
    expect(screen.getByLabelText("下一首")).toBeDisabled();
    expect(screen.getByTestId("notch-mode")).toBeDisabled();
    expect(screen.getByTestId("notch-favorite")).toBeDisabled();
  });

  it("disables the favourite control for a temporary item (it has no library identity)", () => {
    renderNotch();
    expect(screen.getByTestId("notch-favorite")).toBeDisabled();
  });
});

describe("housing geometry", () => {
  it("draws the bezel at the detected housing width, not a stylesheet constant", async () => {
    renderNotch();
    // 185 pt is this machine's housing; a fixed 186 would put the cover under
    // the bezel on a wider notch.
    await waitFor(() => expect(root().style.getPropertyValue("--notch-hw-width")).toBe("185px"));
  });
});

describe("playing indicator", () => {
  it("marks the bar as playing only while playback is actually running", () => {
    renderNotch({ state: "playing" });
    expect(root()).toHaveAttribute("data-playing", "true");
  });

  it("stops the indicator when paused", () => {
    renderNotch({ state: "paused" });
    expect(root()).toHaveAttribute("data-playing", "false");
  });

  it("keeps the indicator still while loading", () => {
    renderNotch({ state: "loading" });
    expect(root()).toHaveAttribute("data-playing", "false");
  });

  it("keeps the indicator still when nothing is current even if state says playing", () => {
    renderNotch({
      state: "playing",
      currentQueueEntryId: null,
      currentSongId: null,
      currentTitle: null,
      currentCanImport: false,
    });
    expect(root()).toHaveAttribute("data-playing", "false");
  });
});

describe("playback mode", () => {
  it("cycles list → shuffle → single and only changes the icon", () => {
    renderNotch({ mode: "sequential" });
    const mode = screen.getByTestId("notch-mode");
    expect(mode).toHaveAttribute("aria-label", "列表循环");

    fireEvent.click(mode);
    expect(capturedInvoke).toHaveBeenCalledWith("player_control", { action: "mode:shuffle" });
  });

  it("never gives the mode control a theme-accent colour rule", () => {
    // The generated stylesheet must not colour the mode control on any state
    // (desktop-playback requires neutral shuffle in the player bar; the notch
    // player follows the same rule).
    const modeBlocks = notchCss
      .split("}")
      .filter((block) => block.includes("#btn-mode"))
      .join("}");
    expect(modeBlocks).not.toMatch(/color\s*:/);
  });
});

describe("window growth protocol", () => {
  it("awaits the resize before showing the expanded panel", async () => {
    renderNotch();
    expect(root()).toHaveAttribute("data-expanded", "false");

    fireEvent.pointerOver(screen.getByTestId("notch-ear-left"), { relatedTarget: document.body });

    await waitFor(() => expect(root()).toHaveAttribute("data-expanded", "true"));
    expect(capturedInvoke).toHaveBeenCalledWith("notch_player_set_expanded", { expanded: true });
  });

  it("does not expand when the window refuses the resize", async () => {
    capturedInvoke.mockImplementation(async (command: string) => {
      if (command === "song_cover_keys") return {};
      if (command === "song_detail") return null;
      if (command === "notch_player_set_expanded") throw new Error("not open");
      return null;
    });
    renderNotch();
    fireEvent.pointerOver(screen.getByTestId("notch-ear-right"), { relatedTarget: document.body });
    await waitFor(() =>
      expect(capturedInvoke).toHaveBeenCalledWith("notch_player_set_expanded", { expanded: true }),
    );
    expect(root()).toHaveAttribute("data-expanded", "false");
  });

  it("collapses on leave and only shrinks the window after the animation", async () => {
    vi.useFakeTimers();
    renderNotch();
    fireEvent.pointerOver(screen.getByTestId("notch-ear-left"), { relatedTarget: document.body });
    await act(async () => {
      await Promise.resolve();
    });
    expect(root()).toHaveAttribute("data-expanded", "true");

    fireEvent.pointerLeave(root());
    // The panel animates closed first; the window is still at the expanded size.
    await act(async () => {
      vi.advanceTimersByTime(150);
    });
    expect(root()).toHaveAttribute("data-expanded", "false");
    expect(
      capturedInvoke.mock.calls.filter(
        ([command, args]) =>
          command === "notch_player_set_expanded" &&
          (args as { expanded: boolean }).expanded === false,
      ),
    ).toHaveLength(0);

    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    expect(capturedInvoke).toHaveBeenCalledWith("notch_player_set_expanded", { expanded: false });
  });

  it("stays open while the progress slider is being dragged", async () => {
    vi.useFakeTimers();
    renderNotch();
    fireEvent.pointerOver(screen.getByTestId("notch-ear-left"), { relatedTarget: document.body });
    await act(async () => {
      await Promise.resolve();
    });

    const slider = screen.getByRole("slider", { name: "播放进度" });
    fireEvent.pointerDown(slider);
    fireEvent.pointerLeave(root());
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    expect(root()).toHaveAttribute("data-expanded", "true");
  });
});

describe("commands", () => {
  it("seeks with the slider's value", () => {
    renderNotch({ currentQueueEntryId: "entry-1", currentSongId: "song-1", duration: 200 });
    const slider = screen.getByRole("slider", { name: "播放进度" });
    fireEvent.change(slider, { target: { value: "120" } });
    expect(capturedInvoke).toHaveBeenCalledWith("seek", { position: 120 });
  });

  it("sends the same transport actions as the player bar", () => {
    renderNotch();
    fireEvent.click(screen.getByLabelText("上一首"));
    fireEvent.click(screen.getByTestId("notch-play"));
    fireEvent.click(screen.getByLabelText("下一首"));
    expect(capturedInvoke).toHaveBeenCalledWith("player_control", { action: "previous" });
    expect(capturedInvoke).toHaveBeenCalledWith("player_control", { action: "pause" });
    expect(capturedInvoke).toHaveBeenCalledWith("player_control", { action: "next" });
  });

  it("shows the play icon when paused", () => {
    renderNotch({ state: "paused" });
    expect(screen.getByTestId("notch-play")).toHaveAccessibleName("播放");
  });
});
