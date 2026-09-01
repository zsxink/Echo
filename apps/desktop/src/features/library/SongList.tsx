/**
 * Virtualized song list + row (task 10.5 / 10.6).
 *
 * A windowed list renders only a viewport-sized slice of the songs so a large
 * library never materializes tens of thousands of DOM rows (task 10.6). Rows
 * are keyed by stable `SongId`; the action menu is bound to the row's id so a
 * scroll never rebinds a menu to the wrong song. Loading / empty / search-empty
 * states are explicit and never confuse an unfinished load with a full list.
 */

import { useRef, useState } from "react";

import type { SongView } from "../../ipc/ipc-types.generated";
import { SongRow } from "./SongRow";

const ROW_HEIGHT = 44;

export interface SongListProps {
  readonly songs: readonly SongView[];
  readonly search: string;
  readonly loading: boolean;
  readonly isLast: boolean;
  readonly readOnly: boolean;
  readonly onLoadMore: () => void;
  readonly onClearSearch: () => void;
  readonly onPlay: (song: SongView) => void;
  readonly onFavorite: (song: SongView, favorite: boolean) => void;
  readonly onOpenMenu: (song: SongView) => void;
}

export function SongList(props: SongListProps) {
  const { songs, search, loading, isLast, readOnly } = props;
  const [scrollTop, setScrollTop] = useState(0);
  const viewportRef = useRef<HTMLDivElement>(null);

  const searching = search.trim().length > 0;

  if (loading && songs.length === 0) {
    return (
      <div className="list-state" role="status" data-testid="list-loading">
        正在载入…
      </div>
    );
  }
  if (songs.length === 0) {
    return (
      <div className="list-state" data-testid="list-empty">
        <p>{searching ? "没有找到匹配歌曲" : "曲库为空"}</p>
        {searching ? (
          <button type="button" className="btn-link" onClick={props.onClearSearch}>
            清除搜索
          </button>
        ) : null}
      </div>
    );
  }

  // Compute the visible slice: viewport height from the scroll container, a
  // small overscan for smoothness. Only those rows and a spacer render.
  const viewportHeight = viewportRef.current?.clientHeight ?? 480;
  const overscan = 6;
  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - overscan);
  const visibleCount = Math.min(
    songs.length - first,
    Math.ceil(viewportHeight / ROW_HEIGHT) + overscan * 2,
  );

  const visibleRows = [];
  for (let i = first; i < first + visibleCount; i++) {
    const song = songs[i];
    visibleRows.push(
      <SongRow
        key={song.id}
        song={song}
        readOnly={readOnly}
        onPlay={() => props.onPlay(song)}
        onFavorite={(fav) => props.onFavorite(song, fav)}
        onOpenMenu={() => props.onOpenMenu(song)}
        style={{ transform: `translateY(${i * ROW_HEIGHT}px)` }}
      />,
    );
  }

  return (
    <div className="song-list" data-testid="song-list">
      <div className="song-header" role="row" aria-hidden="true">
        <span className="song-cell song-cell-title">标题</span>
        <span className="song-cell song-cell-artist">艺人</span>
        <span className="song-cell song-cell-album">专辑</span>
        <span className="song-cell song-cell-duration">时长</span>
        <span className="song-cell song-cell-actions" />
      </div>
      <div
        ref={viewportRef}
        className="song-viewport"
        role="listbox"
        aria-label="歌曲列表"
        onScroll={(event) => {
          setScrollTop(event.currentTarget.scrollTop);
          // Near the bottom → request the next page (task 10.5 keyset).
          const el = event.currentTarget;
          if (el.scrollTop + el.clientHeight >= el.scrollHeight - ROW_HEIGHT * 4) {
            if (!isLast && !loading) props.onLoadMore();
          }
        }}
      >
        <div className="song-virtual-window" style={{ height: `${songs.length * ROW_HEIGHT}px` }}>
          {visibleRows}
        </div>
      </div>
    </div>
  );
}
