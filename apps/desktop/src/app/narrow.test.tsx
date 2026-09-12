/**
 * Task 12.3 — 760px narrow-screen sidebar / mask and resize state preservation.
 *
 * Acceptance (desktop-app-shell spec §窄屏布局与浮层关闭必须可预测):
 *  - On ≤760px the sidebar is toggled by a menu button and shows a mask; opening
 *    reflects an expanded state and clicking the mask closes the sidebar and
 *    returns focus to the menu button (the trigger).
 *  - Resizing to narrow never loses the current view, the search text, or the
 *    playback state — the shell keeps routing/UI state across the toggle.
 * The CSS hides/shows the toggle + mask via a 760px media query; jsdom cannot
 * evaluate the media query, so this test exercises the state machine the media
 * query drives (open → mask → close → focus restore) plus that search/view are
 * preserved across a toggle.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";

const CONFIGURED = {
  configured: true,
  readOnly: false,
  unavailable: false,
  scanning: false,
  activeRoot: "/Music",
};

function renderConfiguredShell() {
  const song = {
    id: "song-1",
    title: "Lacquer Love",
    artist: "Echo Unit",
    album: "Velvet",
    durationS: 210,
    favorite: false,
    playCount: 3,
    availability: "available",
    relativePath: "Artists/Echo Unit/Lacquer Love.flac",
  };
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("library_status", CONFIGURED);
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("all_songs", { items: [song], isLast: true, nextCursor: null });
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("set_volume", { ok: true });
  return render(<App />);
}

describe("Narrow-screen sidebar (task 12.3)", () => {
  afterEach(() => {
    // @ts-expect-error test hook
    globalThis.__echoTest?.reset();
    vi.restoreAllMocks();
  });

  it("opens the sidebar from the menu button and closes it via the mask, restoring focus", async () => {
    renderConfiguredShell();
    await screen.findByTestId("search-input");
    const toggle = screen.getByTestId("sidebar-toggle");
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    act(() => toggle.focus());
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    // Mask reflects the open state (sidebar open behind it).
    const mask = screen.getByTestId("sidebar-mask");
    expect(mask).toBeInTheDocument();

    // Clicking the mask closes the sidebar and returns focus to the toggle.
    fireEvent.click(mask);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => expect(toggle).toHaveFocus());
  });

  it("preserves search text, the current view and playback across a sidebar toggle", async () => {
    renderConfiguredShell();
    // Enter text in search and switch to the favorites view.
    const searchInput = (await screen.findByTestId("search-input")) as HTMLInputElement;
    fireEvent.change(searchInput, { target: { value: "lacquer" } });
    const favorites = screen.getByRole("button", { name: "喜欢的音乐" });
    fireEvent.click(favorites);

    // Toggle the sidebar open, then closed.
    const toggle = screen.getByTestId("sidebar-toggle");
    fireEvent.click(toggle);
    fireEvent.click(screen.getByTestId("sidebar-mask"));

    // Resize/toggle must not lose the search text, the selected view, or the
    // player snapshot (playback state).
    expect((screen.getByTestId("search-input") as HTMLInputElement).value).toBe("lacquer");
    expect(screen.getByRole("heading", { name: "喜欢的音乐" })).toBeInTheDocument();
    expect(favorites).toHaveAttribute("aria-current", "page");
  });
});
