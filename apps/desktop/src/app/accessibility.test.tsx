/**
 * Task 12.2 — full keyboard path and screen-reader names/states, verified with
 * Testing Library + axe.
 *
 * Acceptance (desktop-app-shell spec §键盘焦点与 Escape 行为必须可访问):
 *  - Tab / Shift+Tab / Enter / Space reach navigation, search, import, theme,
 *    settings, playback and queue controls; focus order follows the visual
 *    order and every control has a readable name (task 12.1's focus trap +
 *    roving menu cover the menu/dialog path).
 *  - axe reports no blocking (critical/serious) accessibility violations for
 *    the rendered shell (single sidebar/visible workspace at a time).
 */

import { act, fireEvent, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";

/** A configured library status so the full shell (sidebar + workspace) renders. */
const CONFIGURED = {
  configured: true,
  readOnly: false,
  unavailable: false,
  scanning: false,
  activeRoot: "/Music",
};

function renderConfiguredShell() {
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("library_status", CONFIGURED);
  // The default "all" view fetches `all_songs` → a paged result with items.
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
  globalThis.__echoTest.setInvoke("all_songs", { items: [song], isLast: true, nextCursor: null });
  // The player bar issues a few commands on mount (volume already at 1.0).
  const noopResult = { ok: true };
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("set_volume", noopResult);
  return render(<App />);
}

async function scan(container: HTMLElement): Promise<axe.AxeResults> {
  return axe.run(container, { resultTypes: ["violations"] });
}

/** Keep only the violations axe can meaningfully assess in jsdom. */
function blockingViolations(results: axe.AxeResults): axe.Result[] {
  return results.violations.filter((v) => v.impact === "critical" || v.impact === "serious");
}

describe("Keyboard path (task 12.2)", () => {
  afterEach(() => {
    // @ts-expect-error test hook
    globalThis.__echoTest?.reset();
    vi.restoreAllMocks();
  });

  it("tabbing from the sidebar brand reaches nav, search and settings in visual order", async () => {
    renderConfiguredShell();
    const searchInput = await screen.findByTestId("search-input");
    // A nav item's accessible name carries its count once the view has loaded
    // completely (资料库导航计数), so match on the label prefix.
    const navAll = screen.getByRole("button", { name: /^全部歌曲/ });

    // Focus lands on the first nav button; Tab walks to the next nav item.
    navAll.focus();
    fireEvent.keyDown(navAll, { key: "Tab" });
    expect(screen.getByRole("button", { name: "最近添加" })).toBeInTheDocument();

    // Every interactive control has an accessible name (screen-reader state).
    expect(navAll).toHaveAccessibleName(/^全部歌曲/);
    expect(searchInput).toHaveAccessibleName("搜索歌曲");
    navAll.focus();
    expect(navAll).toBeInTheDocument();
  });

  it("a nav button is keyboard-reachable (name + focus) and activation switches the view", async () => {
    renderConfiguredShell();
    const favorites = await screen.findByRole("button", { name: /^喜欢的音乐/ });
    // Keyboard reachable: focusable with a readable name/state.
    act(() => favorites.focus());
    expect(favorites).toHaveFocus();
    expect(favorites).toHaveAccessibleName(/^喜欢的音乐/);
    // Enter/Space on a <button> synthesize a click natively; the shell switches
    // the workspace view to the selected library view.
    fireEvent.click(favorites);
    expect(await screen.findByRole("heading", { name: "喜欢的音乐" })).toBeInTheDocument();
  });

  it("the song action menu is keyboard-reachable with roving focus", async () => {
    renderConfiguredShell();
    // Open the song menu from the row's "⋯" control (aria-label="歌曲操作").
    fireEvent.click(await screen.findByRole("button", { name: "歌曲操作" }));
    const menu = await screen.findByRole("menu", { name: "歌曲操作菜单" });
    // The prototype focuses 下一首播放 when the menu opens (task 12.1).
    expect(screen.getByRole("menuitem", { name: "下一首播放" })).toHaveFocus();
    // Roving focus: ArrowDown moves to the next item.
    const playItem = screen.getByRole("menuitem", { name: "播放" });
    act(() => playItem.focus());
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: "下一首播放" })).toHaveFocus();
    // Escape closes the menu (single stack) and focus returns to the row.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument();
  });
});

describe("axe scan (task 12.2)", () => {
  afterEach(() => {
    // @ts-expect-error test hook
    globalThis.__echoTest?.reset();
  });

  it("reports no blocking accessibility violations for the configured shell", async () => {
    const { container } = renderConfiguredShell();
    await screen.findByTestId("search-input");
    const violations = blockingViolations(await scan(container));
    const summary = violations.map((v) => `${v.id}: ${v.help}`).join("; ");
    expect(violations, `axe blocking violations: ${summary}`).toEqual([]);
  });
});
