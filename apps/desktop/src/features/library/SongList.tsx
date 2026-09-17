/**
 * Song table (prototype `.table-wrap` + `.track-table`) with windowed rows.
 *
 * The prototype's DOM is reproduced exactly: one `.table-wrap` scroll container
 * holding the `.track-table` (`colgroup` / `thead` / `tbody.track-body`) and the
 * `.empty-results` block that is toggled with the `show` class rather than
 * swapped in.
 *
 * Windowing (task 10.6): a large library must never materialize tens of
 * thousands of rows, so only the viewport slice (plus overscan) renders, padded
 * by two `aria-hidden` spacer rows whose combined height keeps the scrollbar
 * proportional to the real song count. Rows are keyed by stable `SongId`; the
 * action menu is bound to the row's id so scrolling can never rebind it.
 *
 * Loading / search-empty / recoverable-error states are explicit and never
 * confuse an unfinished load with a full library (task 10.7).
 */

import { useRef, useState } from "react";

import type { SongView } from "../../ipc/ipc-types.generated";
import { useCoverKeys } from "../../app/coverArt";
import { SongRow } from "./SongRow";
import type { MenuAnchor } from "./SongMenu";

/** `.track-table td { height: 44px }` — kept in sync with the stylesheet. */
const ROW_HEIGHT = 44;
const OVERSCAN = 6;

export interface SongListProps {
  readonly songs: readonly SongView[];
  readonly search: string;
  readonly loading: boolean;
  readonly isLast: boolean;
  readonly readOnly: boolean;
  /** The currently playing SongId, if any — drives the row playing indicator. */
  readonly currentSongId: string | null;
  /** True while playback is actually running — animates the current row's bars. */
  readonly playing: boolean;
  /** A recoverable load error (task 10.7); existing content is preserved. */
  readonly error?: string | null;
  readonly onRetry?: () => void;
  /** An optional "导入歌曲" entry shown on an empty (unsearched) library. */
  readonly onImport?: () => void;
  readonly onLoadMore: () => void;
  readonly onClearSearch: () => void;
  readonly onPlay: (song: SongView) => void;
  readonly onFavorite: (song: SongView, favorite: boolean) => void;
  readonly onEnqueue: (song: SongView) => void;
  /** Opens the row's `.song-more` menu, anchored to the control that opened it. */
  readonly onOpenMenu: (song: SongView, anchor: MenuAnchor) => void;
}

export function SongList(props: SongListProps) {
  const { songs, search, loading, isLast, readOnly, currentSongId, playing, error, onRetry } =
    props;
  const [scrollTop, setScrollTop] = useState(0);
  const viewportRef = useRef<HTMLDivElement>(null);

  const searching = search.trim().length > 0;

  // Visible window: viewport height from the scroll container, plus overscan.
  // Computed before any early return so the artwork hook below is called on
  // every render, in the same order, in every list state.
  const viewportHeight = viewportRef.current?.clientHeight ?? 520;
  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const visibleCount = Math.min(
    songs.length - first,
    Math.ceil(viewportHeight / ROW_HEIGHT) + OVERSCAN * 2,
  );
  const visible = songs.slice(first, first + visibleCount);

  // Embedded artwork for the rendered window only (design §115). Windowing is
  // what keeps this cheap: a 50k-song library asks about the rows on screen,
  // not the rows that exist.
  const coverKeys = useCoverKeys(visible.map((song) => song.id));

  if (loading && songs.length === 0) {
    return (
      <div className="list-status" role="status" data-testid="list-loading">
        正在载入…
      </div>
    );
  }

  if (songs.length === 0 && error) {
    // Empty *because* the query failed — show the cause + a retry, never a
    // fabricated "empty library".
    return (
      <div className="list-status is-error" data-testid="list-error" role="alert">
        <p>{error}</p>
        {onRetry ? (
          <button type="button" className="btn" onClick={onRetry}>
            重试
          </button>
        ) : null}
      </div>
    );
  }

  return (
    <div
      className="table-wrap"
      data-testid="song-list"
      ref={viewportRef}
      onScroll={(event) => {
        const el = event.currentTarget;
        setScrollTop(el.scrollTop);
        // Near the bottom → request the next page (task 10.5 keyset).
        if (el.scrollTop + el.clientHeight >= el.scrollHeight - ROW_HEIGHT * 4) {
          if (!isLast && !loading) props.onLoadMore();
        }
      }}
    >
      {error && songs.length > 0 ? (
        // Recoverable load error with existing content: a non-destructive banner
        // keeps prior results usable and offers retry (task 10.7).
        <div className="list-banner is-error" role="alert" data-testid="list-banner-error">
          <span>{error}</span>
          {onRetry ? (
            <button type="button" className="btn" onClick={onRetry}>
              重试
            </button>
          ) : null}
        </div>
      ) : null}
      <table className="track-table">
        <colgroup>
          <col className="number" />
          <col className="title" />
          <col className="album" />
          <col className="time" />
          <col className="actions" />
        </colgroup>
        <thead>
          <tr>
            <th>#</th>
            <th>歌曲</th>
            <th className="album">专辑</th>
            <th>时长</th>
            <th aria-label="歌曲操作" />
          </tr>
        </thead>
        <tbody id="track-body">
          {first > 0 ? <tr aria-hidden="true" style={{ height: first * ROW_HEIGHT }} /> : null}
          {visible.map((song, offset) => (
            <SongRow
              key={song.id}
              song={song}
              index={first + offset + 1}
              readOnly={readOnly}
              nowPlaying={currentSongId !== null && song.id === currentSongId}
              playing={playing}
              coverKey={coverKeys.get(song.id) ?? null}
              onPlay={() => props.onPlay(song)}
              onFavorite={(favorite) => props.onFavorite(song, favorite)}
              onEnqueue={() => props.onEnqueue(song)}
              onOpenMenu={(anchor) => props.onOpenMenu(song, anchor)}
            />
          ))}
          {first + visibleCount < songs.length ? (
            <tr
              aria-hidden="true"
              style={{ height: (songs.length - first - visibleCount) * ROW_HEIGHT }}
            />
          ) : null}
        </tbody>
      </table>

      {/* The prototype keeps `.empty-results` inside `.table-wrap` and toggles it
          with `.show` instead of swapping the table out. An unsearched empty
          library offers the import entry point rather than a dead end. */}
      <div className={`empty-results${songs.length === 0 ? " show" : ""}`} data-testid="list-empty">
        <h2>{searching ? "没有找到匹配的音乐" : "曲库为空"}</h2>
        <p>
          {searching
            ? "试试歌名、艺人或专辑中的其他关键词。"
            : "导入本机音乐文件后，Echo 会扫描、整理并在这里列出。"}
        </p>
        {searching ? (
          <button type="button" className="btn" onClick={props.onClearSearch}>
            清除搜索
          </button>
        ) : !readOnly && props.onImport ? (
          <button type="button" className="btn btn-primary" onClick={props.onImport}>
            导入歌曲
          </button>
        ) : null}
      </div>
    </div>
  );
}
