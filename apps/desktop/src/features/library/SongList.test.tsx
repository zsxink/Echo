/**
 * Task 10.6 — windowed song list + playing indicator.
 *
 * Verifies the two acceptance points of "大曲库浏览":
 *  1. A large library never materializes all rows: only the viewport slice
 *     (plus overscan) is rendered into the DOM, and two `aria-hidden` spacer
 *     rows keep the scrollbar proportional to the real song count.
 *  2. The current-playing indicator (播放标识) binds to the correct SongId:
 *     exactly the row whose id equals `currentSongId` reports `aria-current`
 *     and shows the prototype's `.playing-bars`; a scroll must not rebind it.
 *
 * The DOM under test is the prototype's own table (`.table-wrap` →
 * `.track-table` → `.track-row`).
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

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

function renderList(songs: readonly SongView[], currentSongId: string | null, playing = false) {
  return render(
    <SongList
      songs={songs}
      search=""
      loading={false}
      isLast
      readOnly={false}
      currentSongId={currentSongId}
      playing={playing}
      onLoadMore={noop}
      onClearSearch={noop}
      onPlay={noop}
      onFavorite={noop}
      onPlayNext={noop}
      onOpenMenu={noop}
    />,
  );
}

/** Total height of the two `aria-hidden` spacer rows, in px. */
function spacerHeight(container: HTMLElement): number {
  let total = 0;
  for (const row of Array.from(
    container.querySelectorAll<HTMLTableRowElement>("tr[aria-hidden]"),
  )) {
    total += Number.parseFloat(row.style.height || "0");
  }
  return total;
}

describe("SongList windowing (task 10.6)", () => {
  it("renders only a viewport slice of a 50,000-song library, not all rows", () => {
    const songs = makeSongs(50_000);
    const { container } = renderList(songs, null);

    // The prototype's scroll container and table are what is rendered…
    expect(container.querySelector(".table-wrap")).toBeInTheDocument();
    expect(container.querySelector(".track-table")).toBeInTheDocument();
    // …but only a viewport-sized slice of DOM rows materializes.
    const renderedRows = container.querySelectorAll(".track-row");
    expect(renderedRows.length).toBeGreaterThan(0);
    expect(renderedRows.length).toBeLessThan(100);
    expect(renderedRows.length).toBeLessThan(songs.length);
    // The spacer rows still describe the whole list, so the scrollbar is right.
    expect(spacerHeight(container)).toBeGreaterThan(0);
  });

  it("binds the playing indicator to exactly the current SongId", () => {
    const songs = makeSongs(40);
    const current = songs[7];
    const { container } = renderList(songs, current.id);

    // The playing row reports aria-current and carries the prototype's
    // `.selected` state, which is what reveals the playing bars.
    const playingRow = screen.getByTestId(`song-row-${current.id}`);
    expect(playingRow).toHaveAttribute("aria-current", "true");
    expect(playingRow).toHaveClass("selected");
    expect(playingRow.querySelector(".playing-bars")).toBeInTheDocument();

    // Exactly one row is selected, and it is that one — a scroll or a re-render
    // can never rebind the indicator to another song.
    const selected = Array.from(container.querySelectorAll<HTMLElement>(".track-row.selected"));
    expect(selected).toHaveLength(1);
    expect(selected[0].dataset.songId).toBe(current.id);
    for (const row of Array.from(
      container.querySelectorAll<HTMLElement>(".track-row:not(.selected)"),
    )) {
      expect(row).not.toHaveAttribute("aria-current");
    }
  });

  it("animates the current row's bars only while playback is actually running", () => {
    const songs = makeSongs(10);
    const current = songs[3];

    // Paused: the bars are revealed (`.selected`) but frozen — no animation.
    const paused = renderList(songs, current.id, false);
    const pausedRow = screen.getByTestId(`song-row-${current.id}`);
    expect(pausedRow).toHaveClass("selected");
    expect(pausedRow).not.toHaveClass("is-playing");
    paused.unmount();

    // Playing: the stylesheet's `.is-playing` binding makes the bars dance.
    renderList(songs, current.id, true);
    const playingRow = screen.getByTestId(`song-row-${current.id}`);
    expect(playingRow).toHaveClass("selected");
    expect(playingRow).toHaveClass("is-playing");
  });

  it("keeps rows keyed by stable SongId at their absolute position", () => {
    const songs = makeSongs(5000);
    const { container } = renderList(songs, null);
    const firstRow = container.querySelector<HTMLElement>(".track-row");
    expect(firstRow).not.toBeNull();
    expect(firstRow!.dataset.songId).toBe("song-0");
    expect(firstRow!.querySelector(".track-title")?.textContent).toBe("Song 0");
    // The first row carries the table's own numbering, not a transform offset.
    expect(firstRow!.querySelector(".track-number span")?.textContent).toBe("01");
  });

  it("uses the rendered 44px row height when choosing the scrolled window", () => {
    const songs = makeSongs(100);
    const { container } = renderList(songs, null);
    const viewport = screen.getByTestId("song-list");
    // jsdom has no layout engine, so provide the scroll container's viewport
    // height explicitly. At 440px, the first rendered row after six rows of
    // overscan must be song 4: a stale 52px virtual-row value would render song
    // 2 instead, leaving visible controls bound to the wrong song after scroll.
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: 440 });

    fireEvent.scroll(viewport, { target: { scrollTop: 440 } });

    const firstRow = container.querySelector<HTMLElement>(".track-row");
    expect(firstRow).toHaveAttribute("data-song-id", "song-4");
  });

  it("resets its virtual offset after a refresh empties the current page", () => {
    const songs = makeSongs(100);
    const rendered = renderList(songs, null);
    const viewport = screen.getByTestId("song-list");
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: 440 });

    fireEvent.scroll(viewport, { target: { scrollTop: 880 } });
    expect(rendered.container.querySelector(".track-row")).toHaveAttribute(
      "data-song-id",
      "song-14",
    );

    // `useSongs.reset()` temporarily provides an empty page before the first
    // replacement page resolves. The next page must begin at song 0, rather
    // than inheriting the prior list's virtual spacer.
    rendered.rerender(
      <SongList
        songs={[]}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        playing={false}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    rendered.rerender(
      <SongList
        songs={songs}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        playing={false}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );

    expect(rendered.container.querySelector(".track-row")).toHaveAttribute(
      "data-song-id",
      "song-0",
    );
  });

  it("shows the empty state when there are no songs", () => {
    const { container } = renderList([], null);
    const empty = screen.getByTestId("list-empty");
    // The prototype toggles `.show` instead of swapping the table out.
    expect(empty).toHaveClass("empty-results", "show");
    expect(empty).toBeInTheDocument();
    expect(container.querySelectorAll(".track-row").length).toBe(0);
  });

  it("restores a controlled bulk selection after a virtual row leaves and re-enters", () => {
    const songs = makeSongs(100);
    const selectedIds = new Set(["song-0"]);
    const { container } = render(
      <SongList
        songs={songs}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        playing={false}
        selectionMode
        selectedIds={selectedIds}
        onToggleSelection={vi.fn()}
        onToggleSelectAll={vi.fn()}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    expect(screen.getByTestId("song-row-song-0")).toHaveClass("bulk-selected");
    expect(screen.getByTestId("song-select-song-0").querySelector("svg")).toBeInTheDocument();
    expect(screen.getByTestId("song-select-song-1").querySelector("svg")).toBeNull();
    expect(screen.getByLabelText("全选当前已加载歌曲").querySelector("svg")).toBeNull();

    const viewport = screen.getByTestId("song-list");
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: 440 });
    fireEvent.scroll(viewport, { target: { scrollTop: 440 } });
    expect(container.querySelector('[data-testid="song-row-song-0"]')).toBeNull();

    fireEvent.scroll(viewport, { target: { scrollTop: 0 } });
    expect(screen.getByTestId("song-row-song-0")).toHaveClass("bulk-selected");
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
        playing={false}
        error="资料库暂不可用，请重试"
        onRetry={onRetry}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    // Never a fabricated "曲库为空": the cause + retry render instead.
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
        playing={false}
        error="加载歌曲失败，请重试"
        onRetry={onRetry}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    // Existing rows remain visible (not wiped by the error)…
    expect(document.querySelectorAll(".track-row").length).toBe(songs.length);
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
        playing={false}
        onImport={onImport}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
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
        playing={false}
        onImport={noop}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    expect(screen.queryByText("导入歌曲")).not.toBeInTheDocument();
  });
});

/** Render a SongList with mocked viewport metrics, so the locate clamp against
 *  `scrollHeight - clientHeight` sees real sizes (jsdom measures 0 on both).
 *  The getters are spied on `HTMLElement.prototype` *before* render, so the
 *  effect's first run already reads the fixed dimensions; `afterEach` restores
 *  them so no measurement leaks into the following test. */
const ROW_HEIGHT = 44;

function renderLocated(
  songs: readonly SongView[],
  locateSongId: string | null,
  {
    isLast = true,
    loading = false,
    viewportHeight = 440,
    contentHeight,
    onLoadMore = noop,
  }: {
    isLast?: boolean;
    loading?: boolean;
    viewportHeight?: number;
    contentHeight?: number;
    onLoadMore?: () => void;
  } = {},
) {
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(
    contentHeight ?? songs.length * ROW_HEIGHT,
  );
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(viewportHeight);
  const view = render(
    <SongList
      songs={songs}
      search=""
      loading={loading}
      isLast={isLast}
      readOnly={false}
      currentSongId={locateSongId}
      playing={false}
      locateSongId={locateSongId}
      onLoadMore={onLoadMore}
      onClearSearch={noop}
      onPlay={noop}
      onFavorite={noop}
      onPlayNext={noop}
      onOpenMenu={noop}
    />,
  );
  return { view, viewport: screen.getByTestId("song-list") };
}

describe("SongList locate (playlist-search-locate-import 4.2)", () => {
  afterEach(() => vi.restoreAllMocks());

  it("scrolls the matched song row to the top of the visible area", () => {
    const songs = makeSongs(20);
    const { viewport } = renderLocated(songs, songs[7].id);
    // `index * ROW_HEIGHT` — the numeric alignment, never a DOM lookup.
    expect(viewport.scrollTop).toBe(7 * ROW_HEIGHT);
  });

  it("clamps an end-of-list target to the last reachable position", () => {
    const songs = makeSongs(20);
    // A short viewport whose bottom cannot move the last rows to the absolute
    // top: scroll must align to `scrollHeight - clientHeight`.
    const { viewport } = renderLocated(songs, songs[19].id, { viewportHeight: 300 });
    expect(viewport.scrollTop).toBe(20 * ROW_HEIGHT - 300);
  });

  it("keeps the clamp at zero when the list fits inside the viewport", () => {
    const songs = makeSongs(3);
    const { viewport } = renderLocated(songs, songs[2].id, { viewportHeight: 300 });
    expect(viewport.scrollTop).toBe(0);
  });

  it("ignores a null intent and leaves the scroll position untouched", () => {
    const songs = makeSongs(10);
    const { viewport } = renderLocated(songs, null);
    expect(viewport.scrollTop).toBe(0);
  });

  it("pulls the next page when the target is not yet loaded, then scrolls once it lands", () => {
    const loaded = makeSongs(10);
    const onLoadMore = vi.fn();
    // `isLast=false`, `loading=false` → the intent triggers `onLoadMore`, not a
    // scroll. A 44px viewport leaves room for the incoming page to reach the
    // target's absolute top once it is found.
    const { view, viewport } = renderLocated(loaded, "song-12", {
      isLast: false,
      viewportHeight: 44,
      onLoadMore,
    });
    expect(onLoadMore).toHaveBeenCalledTimes(1);
    expect(viewport.scrollTop).toBe(0);

    // The next page lands with the target as song 12; the effect re-runs on the
    // `songs` change and scrolls it to the top.
    const nextSongs = [
      ...loaded,
      ...makeSongs(3).map((song, offset) => ({ ...song, id: `song-${10 + offset}` })),
    ];
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(
      nextSongs.length * ROW_HEIGHT,
    );
    view.rerender(
      <SongList
        songs={nextSongs}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId="song-12"
        playing={false}
        locateSongId="song-12"
        onLoadMore={onLoadMore}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    expect(viewport.scrollTop).toBe(12 * ROW_HEIGHT);
  });

  it("does not pull the next page while a load is already in flight", () => {
    const onLoadMore = vi.fn();
    renderLocated(makeSongs(10), "song-12", { isLast: false, loading: true, onLoadMore });
    expect(onLoadMore).not.toHaveBeenCalled();
  });

  it("leaves a last-page miss untouched (the caller notifies instead)", () => {
    const songs = makeSongs(10);
    const onLoadMore = vi.fn();
    const { viewport } = renderLocated(songs, "absent-song", { isLast: true, onLoadMore });
    expect(viewport.scrollTop).toBe(0);
    expect(onLoadMore).not.toHaveBeenCalled();
  });

  it("reports a found target so the caller can settle the intent", () => {
    const songs = makeSongs(10);
    const onLocateSettled = vi.fn();
    const view = render(
      <SongList
        songs={songs}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={songs[3].id}
        playing={false}
        locateSongId={songs[3].id}
        onLocateSettled={onLocateSettled}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    expect(onLocateSettled).toHaveBeenCalledWith("found");
    view.unmount();
  });

  it("reports a last-page miss as absent so the caller voices it", () => {
    const songs = makeSongs(10);
    const onLocateSettled = vi.fn();
    const view = render(
      <SongList
        songs={songs}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={null}
        playing={false}
        locateSongId="absent-song"
        onLocateSettled={onLocateSettled}
        onLoadMore={noop}
        onClearSearch={noop}
        onPlay={noop}
        onFavorite={noop}
        onPlayNext={noop}
        onOpenMenu={noop}
      />,
    );
    expect(onLocateSettled).toHaveBeenCalledWith("absent");
    view.unmount();
  });
});
