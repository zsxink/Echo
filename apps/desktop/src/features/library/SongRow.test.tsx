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
import { CollectionNavigationProvider } from "./collectionNavigation";

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
    onPlayNext: vi.fn(),
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

/**
 * Renders the row inside the shell's collection-navigation provider (issue
 * #118). Outside a provider the row has no navigation channel, so the link
 * surfaces only exist in this shape.
 */
function renderNavigableRow(overrides: Partial<Parameters<typeof SongRow>[0]> = {}) {
  const openArtist = vi.fn();
  const openAlbum = vi.fn();
  const props = {
    song: makeSong({ artistKey: "陈婧霏", albumKey: "猩红" }),
    index: 1,
    readOnly: false,
    nowPlaying: false,
    playing: false,
    coverKey: null,
    onPlay: vi.fn(),
    onFavorite: vi.fn(),
    onPlayNext: vi.fn(),
    onOpenMenu: vi.fn(),
    ...overrides,
  };
  const view = render(
    <CollectionNavigationProvider openArtist={openArtist} openAlbum={openAlbum}>
      <table>
        <tbody>
          <SongRow {...props} />
        </tbody>
      </table>
    </CollectionNavigationProvider>,
  );
  return { ...view, props, openArtist, openAlbum };
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

    fireEvent.click(screen.getByRole("button", { name: "将心房设为下一首播放" }));
    expect(props.onPlayNext).toHaveBeenCalledTimes(1);

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
    // The anchor alone travels. Which row this was is not expressible as a DOM
    // node here — windowing replaces the `<tr>` between two presses of the same
    // row — so the caller identifies the entry point by the song the list passes
    // it, not by a node (fix-queue-trigger-toggle).
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
    expect(cover?.querySelector("img")?.getAttribute("src")).toBe("cover://cv1-abc123?s=list");
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

describe("SongRow quality badge", () => {
  it.each([
    ["sq", "SQ"],
    ["hq", "HQ"],
  ] as const)("renders the %s badge after the title", (quality, label) => {
    const { container } = renderRow({ song: makeSong({ quality }) });
    const badge = container.querySelector(".track-title .quality-badge");

    expect(badge).not.toBeNull();
    expect(badge).toHaveClass(`q-${quality}`);
    expect(badge).toHaveTextContent(label);
    // The badge is decoration, not a control: no button role is exposed and
    // the plain-title guard still holds.
    expect(screen.queryByRole("button", { name: label })).toBeNull();
  });

  it("does not render a badge nor reserve space when the song has no quality tier", () => {
    const { container } = renderRow();
    expect(container.querySelector(".track-title .quality-badge")).toBeNull();
    expect(container.querySelector(".track-title")).toHaveTextContent("心房");
  });

  it("does not interfere with row playback when clicking the badge", () => {
    const { props } = renderRow({ song: makeSong({ quality: "sq" }) });

    const badge = screen.getByText("SQ");
    fireEvent.click(badge);

    expect(props.onPlay).toHaveBeenCalledTimes(1);
  });

  it("keeps the album column unaffected by the badge", () => {
    const { container } = renderRow({ song: makeSong({ quality: "hq" }) });
    const row = container.querySelector(".track-row");
    const album = row?.querySelector(".track-album");
    expect(album).toHaveTextContent("猩红");
  });
});

describe("SongRow artist/album links (issue #118)", () => {
  it("navigates to the artist without starting playback", () => {
    const { props, openArtist, openAlbum } = renderNavigableRow();

    fireEvent.click(screen.getByRole("button", { name: "打开歌手 陈婧霏" }));

    expect(openArtist).toHaveBeenCalledWith("陈婧霏");
    expect(openAlbum).not.toHaveBeenCalled();
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("navigates to the album without starting playback", () => {
    const { props, openArtist, openAlbum } = renderNavigableRow();

    fireEvent.click(screen.getByRole("button", { name: "打开专辑 猩红" }));

    expect(openAlbum).toHaveBeenCalledWith("猩红");
    expect(openArtist).not.toHaveBeenCalled();
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("treats Enter/Space on a link the same as a click", () => {
    const { props, openArtist, openAlbum } = renderNavigableRow();

    // Native `<button type="button">` is what makes Enter/Space equivalent to a
    // click — jsdom does not synthesise the activation, so the contract is
    // asserted as the element shape plus "the key never reaches the row".
    const artistLink = screen.getByRole("button", { name: "打开歌手 陈婧霏" });
    expect(artistLink.tagName).toBe("BUTTON");
    expect(artistLink).toHaveAttribute("type", "button");
    artistLink.focus();
    expect(artistLink).toHaveFocus();
    fireEvent.keyDown(artistLink, { key: "Enter" });
    fireEvent.keyDown(artistLink, { key: " " });
    fireEvent.click(artistLink);
    expect(openArtist).toHaveBeenCalledWith("陈婧霏");

    const albumLink = screen.getByRole("button", { name: "打开专辑 猩红" });
    expect(albumLink.tagName).toBe("BUTTON");
    expect(albumLink).toHaveAttribute("type", "button");
    albumLink.focus();
    fireEvent.keyDown(albumLink, { key: "Enter" });
    fireEvent.keyDown(albumLink, { key: " " });
    fireEvent.click(albumLink);
    expect(openAlbum).toHaveBeenCalledWith("猩红");

    // The row's own Enter/Space play path never fires for a link gesture.
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("keeps the selection set intact in multi-select mode", () => {
    const onToggleSelection = vi.fn();
    const onContextMenu = vi.fn();
    const { props, openArtist } = renderNavigableRow({
      selectionMode: true,
      bulkSelected: true,
      onToggleSelection,
      onContextMenu,
    });

    fireEvent.click(screen.getByRole("button", { name: "打开歌手 陈婧霏" }));

    expect(openArtist).toHaveBeenCalledWith("陈婧霏");
    expect(onToggleSelection).not.toHaveBeenCalled();
    expect(onContextMenu).not.toHaveBeenCalled();
    expect(props.onPlay).not.toHaveBeenCalled();
  });

  it("renders no link when the read model has no grouping key", () => {
    const { container } = renderNavigableRow({
      song: makeSong({ artist: undefined, album: undefined }),
    });

    expect(screen.queryByRole("button", { name: /打开歌手/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /打开专辑/ })).toBeNull();
    // 未知艺人 stays as the existing placeholder text; a missing album stays blank.
    expect(container.querySelector(".track-artist")).toHaveTextContent("未知艺人");
    expect(container.querySelector(".track-album")).toHaveTextContent("");
  });

  it("degrades to plain text when no navigation provider is installed", () => {
    const { props } = renderRow({ song: makeSong({ artistKey: "陈婧霏", albumKey: "猩红" }) });

    expect(screen.queryByRole("button", { name: /打开歌手/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /打开专辑/ })).toBeNull();

    // The artist text still plays the row, exactly as before the links existed.
    fireEvent.click(screen.getByText("陈婧霏"));
    expect(props.onPlay).toHaveBeenCalledTimes(1);
  });
});
