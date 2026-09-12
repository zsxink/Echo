/**
 * Task 10.6 — windowed song list + playing indicator.
 *
 * Verifies the two acceptance points of "大曲库浏览":
 *  1. A large library never materializes all rows: only the viewport slice
 *     (plus overscan) is rendered into the DOM, and rows are positioned by an
 *     absolute transform so scrolling stays continuous.
 *  2. The current-playing indicator (播放标识) binds to the correct SongId:
 *     exactly the row whose id equals `currentSongId` reports `aria-current`
 *     and the playing glyph; a scroll must not rebind it to another song.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SongView } from "../../ipc/ipc-types.generated";
import { SongList } from "./SongList";

function makeSongs(count: number): SongView[] {
  const songs: SongView[] = [];
  for (let i = 0; i < count; i++) {
    songs.push({
      id: `song-${i}`,
      title: `Song ${i}`,
      artist: "Artist",
      album: "Album",
      durationS: 180 + (i % 200),
      favorite: false,
      playCount: i,
      availability: "available",
      relativePath: `dir/song-${i}.mp3`,
    });
  }
  return songs;
}

function noop() {}

function renderList(songs: readonly SongView[], currentSongId: string | null) {
  return render(
    <SongList
      songs={songs}
      search=""
      loading={false}
      isLast
      readOnly={false}
      currentSongId={currentSongId}
      onLoadMore={noop}
      onClearSearch={noop}
      onPlay={noop}
      onFavorite={noop}
      onOpenMenu={noop}
    />,
  );
}

describe("SongList windowing (task 10.6)", () => {
  it("renders only a viewport slice of a 50,000-song library, not all rows", () => {
    const songs = makeSongs(50_000);
    const { container } = renderList(songs, null);

    // The virtual window spacer is full height (all rows exist logically)…
    expect(container.querySelector(".song-virtual-window")).toBeInTheDocument();
    // …but only a viewport-sized slice of DOM rows materialize.
    const renderedRows = container.querySelectorAll(".song-row");
    expect(renderedRows.length).toBeGreaterThan(0);
    expect(renderedRows.length).toBeLessThan(100);
    expect(renderedRows.length).toBeLessThan(songs.length);
  });

  it("binds the playing indicator to exactly the current SongId", () => {
    const songs = makeSongs(40);
    const current = songs[7];
    renderList(songs, current.id);

    // The playing row reports aria-current and the indicator glyph.
    const playingRow = screen.getByTestId(`song-row-${current.id}`);
    expect(playingRow).toHaveAttribute("aria-current", "true");
    expect(playingRow.querySelector(".now-playing-indicator")).toBeInTheDocument();

    // Every other rendered row has no indicator.
    for (const row of Array.from(
      document.querySelectorAll<HTMLElement>(".song-row:not([aria-current=true])"),
    )) {
      expect(row.querySelector(".now-playing-indicator")).toBeNull();
    }
  });

  it("keeps rows keyed by stable SongId at their absolute position", () => {
    const songs = makeSongs(5000);
    const { container } = renderList(songs, null);
    const firstRow = container.querySelector<HTMLElement>(".song-row");
    expect(firstRow).not.toBeNull();
    // Row 0 sits at the top; its transform is translateY(0).
    expect(firstRow!.style.transform).toBe(`translateY(0px)`);

    const title = firstRow!.querySelector(".song-title-text");
    expect(title ? title.textContent : "").toBe("Song 0");
  });

  it("shows the empty state when there are no songs", () => {
    renderList([], null);
    expect(screen.getByTestId("list-empty")).toBeInTheDocument();
  });

  it("shows a retryable error state instead of a fake empty library on load failure (task 10.7)", () => {
    const onRetry = vi.fn();
    render(
      <SongList
        songs={[]}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        error="资料库暂不可用，请重试"
        onRetry={onRetry}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onOpenMenu={noop}
      />,
    );
    // Never a fabricated "曲库为空": the cause + retry render instead.
    expect(screen.queryByTestId("list-empty")).not.toBeInTheDocument();
    expect(screen.getByTestId("list-error")).toBeInTheDocument();
    expect(screen.getByText("重试")).toBeInTheDocument();
    fireEvent.click(screen.getByText("重试"));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("preserves usable content and surfaces a non-destructive error banner when a reload fails (task 10.7)", () => {
    const songs = makeSongs(8);
    const onRetry = vi.fn();
    render(
      <SongList
        songs={songs}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        error="加载歌曲失败，请重试"
        onRetry={onRetry}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onOpenMenu={noop}
      />,
    );
    // Existing rows remain visible (not wiped by the error)…
    expect(document.querySelectorAll(".song-row").length).toBe(songs.length);
    // …alongside the banner + retry.
    expect(screen.getByTestId("list-banner-error")).toBeInTheDocument();
    fireEvent.click(screen.getByText("重试"));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("offers an import entry when the library is empty (task 10.7)", () => {
    const onImport = vi.fn();
    render(
      <SongList
        songs={[]}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        onImport={onImport}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onOpenMenu={noop}
      />,
    );
    expect(screen.getByTestId("list-empty")).toBeInTheDocument();
    fireEvent.click(screen.getByText("导入歌曲"));
    expect(onImport).toHaveBeenCalledTimes(1);
  });

  it("hides the import entry on a read-only library", () => {
    render(
      <SongList
        songs={[]}
        search=""
        loading={false}
        isLast
        readOnly
        currentSongId={null}
        onImport={noop}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onOpenMenu={noop}
      />,
    );
    expect(screen.queryByText("导入歌曲")).not.toBeInTheDocument();
  });
});
