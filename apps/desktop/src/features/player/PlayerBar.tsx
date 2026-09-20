/**
 * 常驻播放栏 (task 11.1) — the prototype's `footer.playerbar`.
 *
 * Four regions, exactly as `docs/interface-terminology.md` names them:
 *   播放进度条   a window-wide `.progress-line` on the bar's top edge
 *   当前播放区   mini record + title/artist, and the entry point to 沉浸式播放器
 *   传输控制区   收藏 / 上一首 / 播放暂停 / 下一首 / 播放模式
 *   音频与队列区 静音 + 音量滑块 + 播放队列
 *
 * Every value comes from the authoritative `PlayerSnapshot`; a rejected command
 * simply leaves the snapshot unchanged (the rollback lives in the Rust actor).
 *
 * 空播放态: the bar is persistent chrome, so it **always** draws its four
 * regions — an idle player shows an empty 当前播放区 (vinyl placeholder + a
 * muted dash) beside the transport controls instead of collapsing into a
 * "未在播放" sentence, and instead of leaving stale old information behind. The
 * controls stay visible but disabled, because an empty queue has nothing to act
 * on; only the volume and the queue toggle (both independent of the current
 * track) stay live.
 *
 * 内置封面: the artwork embedded in the playing file (design §115 内置优先) is
 * resolved through the same `song_cover_keys` batch lookup the song list uses;
 * a song with no embedded artwork keeps the prototype's vinyl placeholder.
 *
 * The seek range accepts fractional seconds so its thumb follows every frame;
 * arrow keys still seek by 5 seconds, and volume uses `step=0.05` (task 11.8).
 * the *visual* fill is driven by the prototype's own `--progress` / `--volume`
 * custom properties on `.progress-line` / `.volume-line`.
 */

import { useState, type CSSProperties } from "react";

import { assetUrl, bridge } from "../../bridge";
import {
  usePlayerSnapshot,
  usePlayerUi,
  useSmoothPosition,
  playerStore,
} from "../../player/playerStore";
import { useCoverKeys } from "../../app/coverArt";
import { Icon } from "../../app/Icon";
import { formatDuration } from "../library";
import { useSongDetail } from "./useSongDetail";
import { bumpLibraryCount, publishSongUpdate } from "../library";
import type { SongView } from "../../ipc/ipc-types.generated";

export function PlayerBar() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();
  const detail = useSongDetail(snapshot.currentSongId);
  // A cover asset can disappear under a live key (the cache is garbage-collected
  // outside the referenced keep-set). A broken image would be worse than the
  // vinyl placeholder, so a load failure falls back to it — remembered per song
  // so a later track still gets its own attempt.
  //
  // The memory is keyed by `songId ?? coverKey`: a file opened from the file
  // browser has no library id, and keying on `songId` alone compares the
  // initial `null` against that `null` — false — which disabled the artwork of
  // every temporary item while the immersive view (same lookup, `?? coverKey`)
  // drew it fine.
  const [failedCoverId, setFailedCoverId] = useState<string | null>(null);

  const hasCurrent = snapshot.currentQueueEntryId !== null;
  const isTemporary = hasCurrent && snapshot.currentCanImport;
  const songId = snapshot.currentSongId;

  // 内置封面 (design §115 内置优先): one batched lookup per mounted bar, skipped
  // entirely while nothing is current (an empty id list is never requested).
  const coverKeys = useCoverKeys(songId ? [songId] : []);
  const coverKey = songId ? (coverKeys.get(songId) ?? null) : (snapshot.currentCoverKey ?? null);
  const artwork = coverKey && failedCoverId !== (songId ?? coverKey) ? assetUrl(coverKey) : null;

  // rAF-interpolated so the bar/滑块 glide instead of stepping per snapshot.
  const position = useSmoothPosition() ?? 0;
  // mpv can report duration after its paused restore snapshot. The library
  // detail is already authoritative metadata for this song, so use it as a
  // temporary range fallback: restored progress is immediately visible and
  // seekable without waiting for the user to press Play.
  const duration = snapshot.duration ?? detail?.durationS ?? 0;
  const progress = duration > 0 ? Math.min(100, (position / duration) * 100) : 0;
  // The real title/artist once `song_detail` answers; a temporary item carries
  // its own display name in the snapshot and has no library detail at all.
  const title = isTemporary ? snapshot.currentTitle : (detail?.title ?? snapshot.currentTitle);
  // Until `song_detail` answers, the 当前播放区 carries the real playback
  // duration instead of inventing an artist name.
  const subline =
    detail?.artist ?? snapshot.currentArtist ?? (duration > 0 ? formatDuration(duration) : null);

  function command(action: string) {
    // Coarse player control; the Rust coordinator owns the queue + snapshot.
    bridge.fireAndForget("player_control", { action });
  }

  const mode = MODES[snapshot.mode];

  return (
    <footer
      className="playerbar"
      data-testid="playerbar"
      data-playing={snapshot.state === "playing"}
      // The bar always draws its four regions; this flag only marks the idle
      // player for the stylesheet (muted placeholder slot, disabled transport).
      data-empty={hasCurrent ? undefined : "true"}
    >
      <div
        className="progress-line"
        id="progress-line"
        style={{ "--progress": `${progress}%` } as CSSProperties}
      >
        <input
          className="progress"
          id="progress"
          type="range"
          role="slider"
          aria-label="播放进度"
          aria-valuetext={`${formatDuration(position)} / ${formatDuration(duration)}`}
          min={0}
          max={duration || 0}
          step="any"
          value={position}
          disabled={!hasCurrent || duration <= 0}
          onKeyDown={(event) => {
            const delta =
              event.key === "ArrowRight" || event.key === "ArrowUp"
                ? 5
                : event.key === "ArrowLeft" || event.key === "ArrowDown"
                  ? -5
                  : null;
            const target =
              event.key === "Home"
                ? 0
                : event.key === "End"
                  ? duration
                  : delta !== null
                    ? Math.max(0, Math.min(duration, position + delta))
                    : null;
            if (target === null) return;
            event.preventDefault();
            bridge.fireAndForget("seek", { position: target });
          }}
          onChange={(event) =>
            bridge.fireAndForget("seek", { position: Number(event.target.value) })
          }
        />
      </div>

      <div className="player-track">
        <button
          type="button"
          className="player-track-card"
          aria-label="展开当前歌曲详情"
          aria-expanded={ui.immersiveOpen}
          onClick={() => playerStore.setImmersiveOpen(!ui.immersiveOpen)}
          data-testid="now-playing-trigger"
        >
          <div className={`mini-cover${artwork ? " has-image" : ""}`} aria-hidden="true">
            {artwork ? (
              <img src={artwork} alt="" onError={() => setFailedCoverId(songId ?? coverKey)} />
            ) : null}
          </div>
          <div className="player-track-text">
            <b>
              {title ? (
                title
              ) : (
                // 空播放态 — an empty song slot, not a "未在播放" sentence.
                <span className="player-track-placeholder" aria-hidden="true">
                  —
                </span>
              )}
            </b>
            {subline ? <span>{subline}</span> : null}
          </div>
        </button>
        {isTemporary ? (
          <button
            type="button"
            className="btn player-import-button"
            aria-label="导入"
            onClick={() => bridge.fireAndForget("import_current_temporary_file")}
          >
            导入
          </button>
        ) : null}
      </div>

      <div className="controls">
        <div className="control-buttons">
          <button
            type="button"
            className={`control favorite${detail?.favorite ? " active" : ""}`}
            aria-label={detail?.favorite ? "取消喜欢当前歌曲" : "喜欢当前歌曲"}
            aria-pressed={detail?.favorite ?? false}
            disabled={snapshot.currentSongId === null || detail === null}
            onClick={() => {
              if (!snapshot.currentSongId || !detail) return;
              const favorite = !detail.favorite;
              bumpLibraryCount("favorites", favorite ? 1 : -1);
              void bridge
                .call("set_favorite", {
                  songId: snapshot.currentSongId,
                  favorite,
                })
                // Broadcast the committed view so the library rows (and this
                // bar's own detail) adopt the new heart without a re-query.
                .then((committed) => publishSongUpdate(committed as SongView))
                .catch(() => bumpLibraryCount("favorites", favorite ? -1 : 1));
            }}
            data-testid="player-favorite"
          >
            <Icon name="heart" filled={detail?.favorite ?? false} />
          </button>
          {/* Transport stays visible with an empty queue (the bar is persistent
              chrome) but is inert: there is no current entry to act on. Only an
              idle player is disabled — a live one keeps every control usable
              even while `song_detail` is still in flight. */}
          <button
            type="button"
            className="control"
            aria-label="上一首"
            disabled={!hasCurrent}
            onClick={() => command("previous")}
          >
            <Icon name="previous" />
          </button>
          <button
            type="button"
            className="control control-play"
            id="main-play"
            aria-label={snapshot.state === "playing" ? "暂停" : "播放"}
            disabled={!hasCurrent}
            onClick={() => command(snapshot.state === "playing" ? "pause" : "play")}
          >
            <Icon name={snapshot.state === "playing" ? "pause" : "play"} />
          </button>
          <button
            type="button"
            className="control"
            aria-label="下一首"
            disabled={!hasCurrent}
            onClick={() => command("next")}
          >
            <Icon name="next" />
          </button>
          <button
            type="button"
            className="control"
            id="playback-mode"
            aria-label={mode.label}
            title={mode.label}
            aria-pressed={snapshot.mode !== "sequential"}
            data-mode={snapshot.mode}
            onClick={() => command(`mode:${mode.next}`)}
          >
            <Icon name={mode.icon} />
          </button>
        </div>
      </div>

      <div className="player-options">
        <button
          type="button"
          className={`control volume-button${snapshot.muted ? " is-muted" : ""}`}
          aria-label={snapshot.muted ? "取消静音" : "静音"}
          title={snapshot.muted ? "取消静音" : "静音"}
          aria-pressed={snapshot.muted}
          onClick={() => bridge.fireAndForget("toggle_mute")}
        >
          <Icon name={snapshot.muted ? "volumeMuted" : "volume"} />
        </button>
        <div
          className="volume-line"
          style={{ "--volume": `${Math.round(snapshot.volume * 100)}%` } as CSSProperties}
        >
          <input
            className="volume"
            type="range"
            role="slider"
            aria-label="音量"
            aria-valuetext={`音量 ${Math.round(snapshot.volume * 100)}%`}
            min={0}
            max={1}
            step={0.05}
            value={snapshot.volume}
            onChange={(event) =>
              bridge.fireAndForget("set_volume", { volume: Number(event.target.value) })
            }
          />
        </div>
        <button
          type="button"
          className="control"
          aria-label="显示播放队列"
          aria-expanded={ui.queueOpen}
          onClick={() => playerStore.setQueueOpen(!ui.queueOpen)}
          data-testid="queue-trigger"
        >
          <Icon name="queue" />
        </button>
      </div>

      {/* Screen-reader status line (task 11.8) — announces playback state plus
          the current song/mode/mute so assistive technology reflects the live
          media state without tabbing through every control. */}
      <span className="sr-only" aria-live="polite" data-testid="player-status-text">
        {buildStatusText(snapshot)}
      </span>
    </footer>
  );
}

/** 播放模式: label, next mode in the cycle, and its solid glyph.
 *  顺序播放的语义是列表循环（播完回到第一条继续，默认模式）——a library is
 *  meant to keep playing, not to stop after its last song. */
const MODES = {
  sequential: { label: "列表循环", next: "shuffle", icon: "modeSequence" },
  shuffle: { label: "随机播放", next: "repeatOne", icon: "modeShuffle" },
  repeatOne: { label: "单曲循环", next: "sequential", icon: "modeRepeatOne" },
} as const;

function buildStatusText(snapshot: ReturnType<typeof usePlayerSnapshot>): string {
  const stateText =
    snapshot.state === "playing"
      ? "正在播放"
      : snapshot.state === "paused"
        ? "已暂停"
        : snapshot.state === "loading"
          ? "加载中"
          : "已停止";
  const parts = [stateText];
  if (snapshot.currentTitle) parts.push(snapshot.currentTitle);
  parts.push(MODES[snapshot.mode].label + (snapshot.muted ? "，静音" : ""));
  return parts.join("，");
}
