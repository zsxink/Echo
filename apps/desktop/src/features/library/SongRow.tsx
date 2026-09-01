/**
 * One song row in the (virtualized) list (task 10.6).
 *
 * Binds every action (play, favorite, menu) to the row's stable `SongId`. The
 * favorite heart is an independent red semantic and is never hidden by theme;
 * a missing file shows an explicit unavailable state and refuses playback.
 */

import type { CSSProperties } from "react";

import type { SongView } from "../../ipc/ipc-types.generated";

export interface SongRowProps {
  readonly song: SongView;
  readonly readOnly: boolean;
  readonly onPlay: () => void;
  readonly onFavorite: (favorite: boolean) => void;
  readonly onOpenMenu: () => void;
  readonly style?: CSSProperties;
}

/** Format a duration in seconds as m:ss (monospace, task 10.5 duration cell). */
export function formatDuration(seconds?: number): string {
  if (seconds === undefined || !Number.isFinite(seconds)) return "--:--";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export function SongRow({ song, readOnly, onPlay, onFavorite, onOpenMenu, style }: SongRowProps) {
  const missing = song.availability === "missing" || song.availability === "pending-delete";
  const paused = song.availability === "pending-delete";

  return (
    <div
      className={`song-row${missing ? " is-missing" : ""}${paused ? " is-pending-delete" : ""}`}
      role="option"
      aria-label={`${song.title ?? "未命名歌曲"} — ${song.artist ?? "未知艺人"}`}
      data-song-id={song.id}
      style={style}
    >
      <button
        type="button"
        className="song-cell song-cell-title songbtn"
        onClick={() => (missing ? undefined : onPlay())}
        disabled={missing}
        aria-disabled={missing}
      >
        <span className="song-title-text">{song.title ?? "未命名歌曲"}</span>
        {missing ? <span className="availability-tag">不可用</span> : null}
      </button>
      <span className="song-cell song-cell-artist">{song.artist ?? "未知艺人"}</span>
      <span className="song-cell song-cell-album">{song.album ?? ""}</span>
      <span className="song-cell song-cell-duration">{formatDuration(song.durationS)}</span>
      <span className="song-cell song-cell-actions" style={{ display: "inline-flex", gap: 4 }}>
        <button
          type="button"
          className="icon-btn favorite-btn"
          aria-pressed={song.favorite}
          aria-label={song.favorite ? "取消收藏" : "收藏"}
          title={song.favorite ? "取消收藏" : "收藏"}
          onClick={() => onFavorite(!song.favorite)}
          disabled={readOnly || missing}
        >
          {song.favorite ? "♥" : "♡"}
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-label="歌曲操作"
          title="歌曲操作"
          onClick={onOpenMenu}
          disabled={readOnly && false}
        >
          ⋯
        </button>
      </span>
    </div>
  );
}
