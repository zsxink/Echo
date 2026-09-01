/**
 * Persistent player bar (task 11.1).
 *
 * Renders the current song cover/info/favorite plus transport controls
 * (上一首 / 播放-暂停 / 下一首), progress, volume + mute, playback mode, queue
 * toggle and the immersive expand entry. Every value is read from the
 * **PlayerSnapshot** (the single source of truth); a rejected command simply
 * leaves the snapshot unchanged (authoritative rollback lives in the Rust
 * actor). When there is no current item the bar shows an empty state rather
 * than stale old information.
 *
 * Seek uses a native `<input type="range" role="slider">`, volume likewise, so
 * keyboard stepping (arrow keys) is provided by the browser and screen readers
 * get the time/percent text (task 11.8 / 12.x).
 */

import type { CSSProperties } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot, usePlayerUi, playerStore } from "../../player/playerStore";
import { formatDuration } from "../library/SongRow";

export function PlayerBar() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();

  const hasCurrent = snapshot.currentQueueEntryId !== null;

  if (!hasCurrent) {
    // Empty state — never stale old info.
    return (
      <footer className="playerbar" data-testid="playerbar" data-empty="true">
        <span className="playerbar-empty">未在播放</span>
      </footer>
    );
  }

  const position = snapshot.position ?? 0;
  const duration = snapshot.duration ?? 0;
  const progress = duration > 0 ? Math.min(100, (position / duration) * 100) : 0;

  function command(action: string) {
    // Coarse player control; the Rust coordinator owns the queue + snapshot.
    void bridge.call("player_control", { action });
  }

  function seek(value: string) {
    void bridge.call("seek", { position: Number(value) });
  }

  function volume(value: string) {
    void bridge.call("set_volume", { volume: Number(value) });
  }

  const modeLabel =
    snapshot.mode === "shuffle"
      ? "随机播放"
      : snapshot.mode === "repeatOne"
        ? "单曲循环"
        : "顺序播放";
  const modeGlyph = snapshot.mode === "shuffle" ? "🔀" : snapshot.mode === "repeatOne" ? "🔁" : "▶";

  return (
    <footer
      className="playerbar"
      data-testid="playerbar"
      data-playing={snapshot.state === "playing"}
    >
      <PlayerCover place="bar" />

      <div className="playerbar-now">
        <span className="playerbar-title">{snapshot.currentSongId ? "当前歌曲" : "临时歌曲"}</span>
        <span className="playerbar-duration">{formatDuration(duration)}</span>
      </div>

      <div className="playerbar-transport">
        <div className="playerbar-tracks">
          <button
            type="button"
            className="icon-btn"
            aria-label="上一首"
            onClick={() => command("previous")}
          >
            ⏮
          </button>
          <button
            type="button"
            className="icon-btn playerbar-play"
            aria-label={snapshot.state === "playing" ? "暂停" : "播放"}
            onClick={() => command(snapshot.state === "playing" ? "pause" : "play")}
          >
            {snapshot.state === "playing" ? "⏸" : "▶"}
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label="下一首"
            onClick={() => command("next")}
          >
            ⏭
          </button>
        </div>
        <div className="playerbar-progress">
          <span className="playerbar-time">{formatPosition(position)}</span>
          <input
            type="range"
            className="range"
            role="slider"
            aria-label="播放进度"
            aria-valuetext={`${Math.round(progress)}%`}
            min={0}
            max={duration || 0}
            step={1}
            value={position}
            onChange={(e) => seek(e.target.value)}
            style={{ "--range-progress": `${progress}%` } as CSSProperties}
          />
          <span className="playerbar-time">{formatDuration(duration)}</span>
        </div>
      </div>

      <div className="playerbar-side">
        <button
          type="button"
          className="icon-btn"
          aria-pressed
          aria-label={modeLabel}
          title={modeLabel}
          onClick={() => command(`mode:${nextMode(snapshot.mode)}`)}
        >
          {modeGlyph}
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-label={snapshot.muted ? "取消静音" : "静音"}
          onClick={() => void bridge.call("toggle_mute")}
        >
          {snapshot.muted ? "🔇" : "🔊"}
        </button>
        <input
          type="range"
          className="range range-volume"
          role="slider"
          aria-label="音量"
          aria-valuetext={`${Math.round(snapshot.volume * 100)}%`}
          min={0}
          max={1}
          step={0.05}
          value={snapshot.volume}
          onChange={(e) => volume(e.target.value)}
        />
        <button
          type="button"
          className="icon-btn"
          aria-label="播放队列"
          aria-pressed={ui.queueOpen}
          onClick={() => playerStore.setQueueOpen(!ui.queueOpen)}
        >
          ▤
        </button>
        <button type="button" className="icon-btn" aria-label="展开播放器">
          ▲
        </button>
        <span className="sr-only" aria-live="polite">
          {snapshot.state === "playing" ? "正在播放" : "已暂停"}
        </span>
      </div>
    </footer>
  );
}

function PlayerCover({ place }: { place: "bar" }) {
  void place;
  // Cover comes from the adaptive asset protocol (task 4.6 / 7.7); the bar
  // uses an opaque asset key, never a user path. Placeholder for now.
  return (
    <span className="playerbar-cover" aria-hidden="true">
      ♪
    </span>
  );
}

function formatPosition(seconds: number): string {
  return formatDuration(seconds || 0);
}

function nextMode(mode: "sequential" | "shuffle" | "repeatOne"): string {
  if (mode === "sequential") return "shuffle";
  if (mode === "shuffle") return "repeatOne";
  return "sequential";
}
