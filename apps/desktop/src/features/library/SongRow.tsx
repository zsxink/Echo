/**
 * One song row of the library table (prototype `.track-row`).
 *
 * Mirrors the prototype's row anatomy: `序号 / 歌曲（封面 + 歌名 + 艺人）/ 专辑 /
 * 时长 / 行内操作`. The number cell is also where playback shows: the playing
 * row gains `.selected`, hides its number and animates the three playing bars —
 * the prototype's only in-table playback indicator, bound to the row's stable
 * SongId so scrolling can never rebind it to another song.
 *
 * Playback binding follows the prototype exactly: the **row itself** is the play
 * target. A click anywhere in the row that does not land on one of the row's own
 * controls starts the song, and the row is focusable (`tabindex="0"`) so
 * Enter/Space do the same — the prototype's `row.addEventListener('click' …)`
 * + `tabindex` pair. The title is therefore a plain `<div class="track-title">`
 * like the prototype's, not a button: a `<button>` there inherits the platform
 * button background/border, which paints a box behind the song name.
 *
 * Deviations from the prototype, both deliberate:
 *  - Real-library availability states (`missing` / `pending-delete`) add an
 *    explicit 不可用 tag, refuse playback and leave the row out of the tab
 *    order; the prototype only ever shows playable sample rows.
 *  - The prototype sets `aria-selected` on the row; here the same fact is
 *    published as `aria-current`, so the playing row is announced without
 *    claiming the table implements the ARIA grid selection model.
 */

import { useState, type KeyboardEvent, type MouseEvent } from "react";

import type { SongView } from "../../ipc/ipc-types.generated";
import { assetUrl } from "../../bridge";
import { Icon } from "../../app/Icon";
import type { MenuAnchor } from "./SongMenu";
import { coverClass } from "./coverClass";

export interface SongRowProps {
  readonly song: SongView;
  /** 1-based position in the view — rendered zero-padded like the prototype. */
  readonly index: number;
  readonly readOnly: boolean;
  /** True when this row is the currently playing song (task 10.6 播放标识). */
  readonly nowPlaying: boolean;
  /**
   * True when playback is actually running (not paused). The number cell
   * becomes the three-bar indicator on the playing row; only a *running*
   * playback animates the bars — a paused song keeps them frozen.
   */
  readonly playing: boolean;
  /** True when this row belongs to the current bulk-selection set. */
  readonly bulkSelected?: boolean;
  /** Selection controls are shown only after the workspace enters multi-select mode. */
  readonly selectionMode?: boolean;
  /**
   * The opaque cover-asset key of this song's **embedded** artwork, or `null`
   * when the file carries none (design §115 内置优先). The prototype's palette
   * placeholder is what a song without artwork renders — never an empty box.
   */
  readonly coverKey?: string | null;
  readonly onPlay: () => void;
  readonly onFavorite: (favorite: boolean) => void;
  readonly onEnqueue: () => void;
  readonly onToggleSelection?: () => void;
  /** Opens the selection-aware menu without triggering playback. */
  readonly onContextMenu?: (anchor: MenuAnchor) => void;
  /** The menu anchors to the control that opened it, as the prototype does. */
  readonly onOpenMenu: (anchor: MenuAnchor) => void;
}

/** Format a duration in seconds as m:ss (player bar, chapters). */
export function formatDuration(seconds?: number | null): string {
  if (seconds === undefined || seconds === null || !Number.isFinite(seconds)) return "--:--";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** The table's `04:17` form — minutes zero-padded, as the prototype shows. */
export function formatTrackDuration(seconds?: number | null): string {
  if (seconds === undefined || seconds === null || !Number.isFinite(seconds)) return "--:--";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
}

/**
 * True when the event originated inside one of the row's own controls.
 *
 * The prototype guards both bindings with `!event.target.closest('button')` so
 * that 收藏 / 加入队列 / 歌曲操作 / 播放 keep their own behaviour instead of also
 * starting the song.
 */
function fromRowControl(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest("button") !== null;
}

export function SongRow({
  song,
  index,
  readOnly,
  nowPlaying,
  playing,
  coverKey,
  bulkSelected = false,
  selectionMode = false,
  onPlay,
  onFavorite,
  onEnqueue,
  onToggleSelection,
  onContextMenu,
  onOpenMenu,
}: SongRowProps) {
  const missing = song.availability === "missing";
  const pendingDelete = song.availability === "pending-delete";
  const unavailable = missing || pendingDelete;
  const title = song.title ?? "未命名歌曲";
  const artist = song.artist ?? "未知艺人";
  // A cover asset can disappear under a live key (the cache is garbage-collected
  // outside the referenced keep-set). A broken image would be worse than the
  // prototype's placeholder, so the first load failure falls back to it.
  const [artworkUnavailable, setArtworkUnavailable] = useState(false);
  const artwork = coverKey && !artworkUnavailable ? assetUrl(coverKey) : null;

  const play = () => {
    if (unavailable) return;
    onPlay();
  };

  return (
    <tr
      className={[
        "track-row",
        nowPlaying ? "selected" : "",
        bulkSelected ? "bulk-selected" : "",
        // The stylesheet gates the bars' animation on `.is-playing`, so the
        // indicator only dances while playback actually runs (frozen when
        // paused) — the prototype's `.is-playing` binding.
        nowPlaying && playing ? "is-playing" : "",
        missing ? "is-missing" : "",
        pendingDelete ? "is-pending-delete" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      aria-current={nowPlaying ? "true" : undefined}
      aria-selected={selectionMode ? bulkSelected : undefined}
      // The prototype's row is the keyboard play path (`tabindex="0"` +
      // Enter/Space). An unplayable row is not a target, so it stays out of the
      // tab order rather than being focusable and inert.
      tabIndex={unavailable ? -1 : 0}
      onClick={(event: MouseEvent<HTMLTableRowElement>) => {
        if (fromRowControl(event.target)) return;
        play();
      }}
      onKeyDown={(event: KeyboardEvent<HTMLTableRowElement>) => {
        if (
          onContextMenu &&
          (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey))
        ) {
          event.preventDefault();
          onContextMenu?.(anchorOf(event.currentTarget));
          return;
        }
        if (event.key !== "Enter" && event.key !== " ") return;
        if (fromRowControl(event.target)) return;
        event.preventDefault();
        play();
      }}
      onContextMenu={(event) => {
        if (!onContextMenu) return;
        event.preventDefault();
        onContextMenu?.(pointerAnchorOf(event.clientX, event.clientY));
      }}
      data-song-id={song.id}
      data-testid={`song-row-${song.id}`}
    >
      {selectionMode ? (
        <td className="selection-column">
          <button
            type="button"
            className={`row-select${bulkSelected ? " active" : ""}`}
            aria-label={bulkSelected ? `取消选择${title}` : `选择${title}`}
            aria-pressed={bulkSelected}
            data-testid={`song-select-${song.id}`}
            onClick={(event) => {
              event.stopPropagation();
              onToggleSelection?.();
            }}
          >
            {bulkSelected ? <Icon name="check" /> : null}
          </button>
        </td>
      ) : null}
      <td className="track-number">
        <span>{String(index).padStart(2, "0")}</span>
        <button
          type="button"
          className="track-play"
          aria-label={`播放${title}`}
          disabled={unavailable}
          onClick={(event) => {
            // The prototype stops the propagation so the row handler does not
            // fire a second time for the same gesture.
            event.stopPropagation();
            play();
          }}
        >
          <Icon name="play" />
        </button>
        <span className="playing-bars" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
      </td>

      <td>
        <div className="track-main">
          {/* The prototype swaps the palette placeholder for the artwork by
              adding `.has-image` and nesting the `<img>` (see `.cover.has-image`
              in the sheet); the tint class stays either way so a late-arriving
              artwork never reflows the row. */}
          <div
            className={`cover ${coverClass(song.id)}${artwork ? " has-image" : ""}`}
            aria-hidden="true"
          >
            {artwork ? (
              <img src={artwork} alt="" onError={() => setArtworkUnavailable(true)} />
            ) : null}
          </div>
          <div className="track-text">
            <div className="track-title">
              <span className="track-title-text">{title}</span>
              {song.quality ? (
                <span className={`quality-badge q-${song.quality}`} aria-hidden="true">
                  {song.quality.toUpperCase()}
                </span>
              ) : null}
            </div>
            <div className="track-artist">
              {artist}
              {unavailable ? (
                <span className="availability-tag">{missing ? "不可用" : "待删除"}</span>
              ) : null}
            </div>
          </div>
        </div>
      </td>

      <td className="album">
        <div className="track-album">{song.album ?? ""}</div>
      </td>

      <td className="track-time">{formatTrackDuration(song.durationS)}</td>

      <td>
        <div className="row-actions">
          <button
            type="button"
            className={`row-action favorite${song.favorite ? " active" : ""}`}
            aria-pressed={song.favorite}
            aria-label={song.favorite ? `取消喜欢${title}` : `喜欢${title}`}
            disabled={readOnly || unavailable}
            onClick={() => onFavorite(!song.favorite)}
          >
            <Icon name="heart" filled={song.favorite} />
          </button>
          <button
            type="button"
            className="row-action add-queue"
            aria-label={`将${title}加入播放队列`}
            disabled={readOnly || unavailable}
            onClick={onEnqueue}
          >
            <Icon name="plus" />
          </button>
          <button
            type="button"
            className="row-action song-more"
            aria-label="歌曲操作"
            title="歌曲操作"
            onClick={(event: MouseEvent<HTMLButtonElement>) => {
              const box = event.currentTarget.getBoundingClientRect();
              onOpenMenu({ top: box.top, right: box.right, bottom: box.bottom, left: box.left });
            }}
          >
            <Icon name="more" />
          </button>
        </div>
      </td>
    </tr>
  );
}

function anchorOf(element: HTMLElement): MenuAnchor {
  const box = element.getBoundingClientRect();
  return { top: box.top, right: box.right, bottom: box.bottom, left: box.left };
}

function pointerAnchorOf(clientX: number, clientY: number): MenuAnchor {
  return {
    top: clientY,
    right: clientX,
    bottom: clientY,
    left: clientX,
    kind: "pointer",
  };
}
