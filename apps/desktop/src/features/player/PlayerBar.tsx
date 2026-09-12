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

import { useState, type CSSProperties } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot, usePlayerUi, playerStore } from "../../player/playerStore";
import { formatDuration } from "../library/SongRow";

export function PlayerBar() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();
  const [importing, setImporting] = useState(false);
  const [importResult, setImportResult] = useState<string | null>(null);

  const hasCurrent = snapshot.currentQueueEntryId !== null;
  const isTemporary = hasCurrent && snapshot.currentCanImport;

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

  async function importToLibrary() {
    if (importing) return;
    setImporting(true);
    setImportResult(null);
    try {
      const result = (await bridge.call("import_current_temporary_file")) as {
        kind: string;
        songId?: string;
        existingSongId?: string;
      };
      switch (result.kind) {
        case "imported":
          setImportResult("已导入到资料库");
          break;
        case "duplicate":
          setImportResult("资料库已有相同歌曲");
          break;
        case "unsupported":
          setImportResult("不支持的格式");
          break;
        case "failed":
          setImportResult("导入失败");
          break;
        default:
          setImportResult("导入完成");
      }
    } catch {
      setImportResult("导入失败");
    } finally {
      setImporting(false);
    }
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
        <span className="playerbar-title">
          {isTemporary ? (
            <>
              <span className="playerbar-temporary-tag" aria-label="临时播放项">
                临时
              </span>{" "}
              {snapshot.currentTitle ?? "临时歌曲"}
            </>
          ) : (
            (snapshot.currentTitle ?? "当前歌曲")
          )}
        </span>
        {isTemporary ? (
          <button
            type="button"
            className="btn btn-sm playerbar-import-btn"
            disabled={importing}
            aria-label="导入到资料库"
            onClick={() => void importToLibrary()}
          >
            {importing ? "导入中…" : (importResult ?? "导入到资料库")}
          </button>
        ) : (
          <span className="playerbar-duration">{formatDuration(duration)}</span>
        )}
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
            aria-valuetext={`${formatPosition(position)} / ${formatDuration(duration)}`}
            min={0}
            max={duration || 0}
            step={5}
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
          aria-valuetext={`音量 ${Math.round(snapshot.volume * 100)}%`}
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
        <button
          type="button"
          className="icon-btn"
          aria-label="展开播放器"
          aria-pressed={ui.immersiveOpen}
          onClick={() => playerStore.setImmersiveOpen(true)}
        >
          ▲
        </button>
        <span className="sr-only" aria-live="polite" data-testid="player-status-text">
          {buildStatusText(snapshot)}
        </span>
      </div>
    </footer>
  );
}

/**
 * Screen-reader status line for the player bar (task 11.8). Announces playback
 * state plus the current song/mode/mute so assistive technology reflects the
 * live media state without the user tabbing through every control.
 */
function buildStatusText(snapshot: ReturnType<typeof usePlayerSnapshot>): string {
  const stateText =
    snapshot.state === "playing"
      ? "正在播放"
      : snapshot.state === "paused"
        ? "已暂停"
        : snapshot.state === "loading"
          ? "加载中"
          : "已停止";
  const song = snapshot.currentTitle;
  const modeText =
    snapshot.mode === "shuffle"
      ? "随机播放"
      : snapshot.mode === "repeatOne"
        ? "单曲循环"
        : "顺序播放";
  const muteText = snapshot.muted ? "，静音" : "";
  const parts = [stateText];
  if (song) parts.push(song);
  parts.push(modeText + muteText);
  return parts.join("，");
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
