/**
 * SongRow — the prototype's row is the play target.
 *
 * Two regressions are locked here, both reported against the real UI:
 *
 *  1. The song name must render as plain text. It was a `<button>` (for a
 *     keyboard play path), which inherits the platform's button background and
 *     border — so the name painted inside a box the prototype never draws. The
 *     prototype's own keyboard path is the *row* (`tabindex="0"` +
 *     Enter/Space), so the row carries it instead.
 *  2. Clicking a row must start the song. Only the invisible `.track-play`
 *     control and the title were bound, so a click anywhere else in the row did
 *     nothing at all.
 *
 * Plus the artwork contract (design §115 内置优先): a resolved embedded-cover
 * key renders the prototype's `.cover.has-image` + `<img>`, and a song whose
 * file carries none keeps the palette placeholder.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SongView } from "../../ipc/ipc-types.generated";
import { coverClass } from "./coverClass";
import { SongRow } from "./SongRow";

function makeSong(extra: Partial<SongView> = {}): SongView {
  return {
    id: "song-1",
    title: "心房",
    artist: "陈婧霏",
    album: "猩红",
    durationS: 257,
    favorite: false,
    playCount: 0,
    availability: "available",
    relativePath: "心房.mp3",
    ...extra,
  };
}

function renderRow(overrides: Partial<Parameters<typeof SongRow>[0]> = {}) {
  const props = {
    song: makeSong(),
    index: 1,
    readOnly: false,
    nowPlaying: false,
    playing: false,
    coverKey: null,
    onPlay: vi.fn(),
    onFavorite: vi.fn(),
    onEnqueue: vi.fn(),
    onOpenMenu: vi.fn(),
    ...overrides,
  };
  const view = render(
    <table>
      <tbody>
        <SongRow {...props} />
      </tbody>
    </table>,
  );
  return { ...view, props };
}

describe("SongRow playback binding", () => {
  it("starts the song when the row is clicked anywhere that is not a control", () => {
    const { props } = renderRow();

    fireEvent.click(screen.getByTestId("song-row-song-1"));
    expect(props.onPlay).toHaveBeenCalledTimes(1);

    // The name is the most likely target: it must play too.
    fireEvent.click(screen.getByText("心房"));
    expect(props.onPlay).toHaveBeenCalledTimes(2);
  });

  it("leaves the row's own controls to their own behaviour", () => {
    const { props } = renderRow();

    fireEvent.click(screen.getByRole("button", { name: "喜欢心房" }));
    expect(props.onFavorite).toHaveBeenCalledWith(true);
    expect(props.onPlay).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "将心房加入播放队列" }));
    expect(props.onEnqueue).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "歌曲操作" }));
    expect(props.onOpenMenu).toHaveBeenCalledTimes(1);

    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("opens the selection-aware menu from the context menu without playing", () => {
    const onContextMenu = vi.fn();
    const { props } = renderRow({ onContextMenu });
    const row = screen.getByTestId("song-row-song-1");

    fireEvent.contextMenu(row, { clientX: 120, clientY: 230 });

    expect(onContextMenu).toHaveBeenCalledTimes(1);
    expect(onContextMenu).toHaveBeenCalledWith({
      top: 230,
      right: 120,
      bottom: 230,
      left: 120,
      kind: "pointer",
    });
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("keeps the selection control separate from row playback", () => {
    const onToggleSelection = vi.fn();
    const { props } = renderRow({ onToggleSelection, bulkSelected: true, selectionMode: true });
    const select = screen.getByTestId("song-select-song-1");

    expect(select).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(select);

    expect(onToggleSelection).toHaveBeenCalledTimes(1);
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("opens the batch menu from the keyboard menu key", () => {
    const onContextMenu = vi.fn();
    const { props } = renderRow({ onContextMenu });
    const row = screen.getByTestId("song-row-song-1");
    row.focus();

    fireEvent.keyDown(row, { key: "ContextMenu" });

    expect(onContextMenu).toHaveBeenCalledTimes(1);
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("plays from the keyboard, the way the prototype's tabbable row does", () => {
    const { props } = renderRow();
    const row = screen.getByTestId("song-row-song-1");

    expect(row).toHaveAttribute("tabindex", "0");
    row.focus();

    fireEvent.keyDown(row, { key: "Enter" });
    expect(props.onPlay).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(row, { key: " " });
    expect(props.onPlay).toHaveBeenCalledTimes(2);

    // A key pressed inside a control belongs to that control.
    fireEvent.keyDown(screen.getByRole("button", { name: "喜欢心房" }), { key: "Enter" });
    expect(props.onPlay).toHaveBeenCalledTimes(2);
  });

  it("is neither focusable nor playable when the file is unavailable", () => {
    const { props } = renderRow({ song: makeSong({ availability: "missing" }) });
    const row = screen.getByTestId("song-row-song-1");

    expect(row).toHaveAttribute("tabindex", "-1");
    expect(row).toHaveClass("is-missing");
    expect(screen.getByText("不可用")).toBeInTheDocument();

    fireEvent.click(row);
    fireEvent.keyDown(row, { key: "Enter" });
    expect(props.onPlay).not.toHaveBeenCalled();
  });
});

describe("SongRow title and artwork", () => {
  it("renders the song name as plain text, not as a control with platform chrome", () => {
    const { container } = renderRow();
    const title = container.querySelector(".track-title");

    expect(title?.tagName).toBe("DIV");
    expect(title).toHaveTextContent("心房");
    // Regression guard: a title button would expose the name as a button name.
    expect(screen.queryByRole("button", { name: "心房" })).toBeNull();
  });

  it("shows the embedded artwork in the prototype's .cover.has-image shape", () => {
    const { container } = renderRow({ coverKey: "cv1-abc123" });
    const cover = container.querySelector(".cover");

    expect(cover).not.toBeNull();
    expect(cover).toHaveClass("has-image");
    // The key is opaque and composes into the cover:// scheme — never a path.
    expect(cover?.querySelector("img")?.getAttribute("src")).toBe("cover://cv1-abc123");
  });

  it("keeps the palette placeholder when the file carries no embedded artwork", () => {
    const { container } = renderRow({ coverKey: null });
    const cover = container.querySelector(".cover");

    expect(cover).not.toHaveClass("has-image");
    expect(cover?.querySelector("img")).toBeNull();
    // The tint class is what makes a coverless row read as data, not as an
    // empty box (brand §6).
    expect(cover?.className).toContain(coverClass("song-1"));
  });

  it("falls back to the placeholder when the artwork cannot be loaded", () => {
    const { container } = renderRow({ coverKey: "cv1-gone" });
    const image = container.querySelector("img");
    expect(image).not.toBeNull();

    fireEvent.error(image as HTMLImageElement);

    const cover = container.querySelector(".cover");
    expect(cover).not.toHaveClass("has-image");
    expect(cover?.querySelector("img")).toBeNull();
  });
});
