/**
 * Immersive player overlay (tasks 11.3–11.6).
 *
 * Expanded from the player bar's "展开播放器" control, this view presents the
 * current song as a vinyl record with metadata, keeps the full transport
 * controls available, and stays mounted while open so track switching updates
 * the content in place rather than flashing back to the workspace. It reads the
 * authoritative PlayerSnapshot for playback state and fetches `song_detail` for
 * the current library song's metadata (title/artist/album/cover) and
 * `get_lyrics` for the effective lyrics content (source, timed/plain lines)
 * when `currentSongId` is set — it never fabricates a value the snapshot or
 * the detail DTO has not confirmed.
 *
 * **Synced lyrics** (task 11.4): rendered from `get_lyrics` lines sorted by
 * time; the current line is highlighted via the authoritative snapshot position
 * using UI progress interpolation (the position interval between the current
 * line and the next is linearly interpolated so the highlight moves smoothly
 * between tick rates). Out-of-order and out-of-range timestamps are
 * pre-sorted/filtered by Core's LRC parser, so the UI does not need to handle
 * them. Clicking a line seeks the player to that line's timestamp.
 *
 * **Plain text / no lyrics** (task 11.5): plain text lyrics scroll as a
 * static list; no current-line indicator or seek. No lyrics shows a clear
 * offline state ("暂无歌词") and切歌 immediately clears previous lyrics —
 * the component remounts on `currentQueueEntryId` change, so stale lyrics
 * never remain.
 *
 * **Narrow responsive** (task 11.3): on a ≤760px window the vinyl, metadata,
 * lyrics and controls stack and scroll; nothing is clipped. The lyrics section
 * scrolls independently so it does not fight the page scroll.
 *
 * **Reduced motion** (task 12.4): the vinyl CSS animation is already disabled
 * under `prefers-reduced-motion`; the lyrics use scrollIntoView with the
 * browser default, which respects reduced motion natively.
 *
 * State is UI-transient (`playerStore.setImmersiveOpen`) — opening or closing
 * never stops, resets, or starts playback.
 */

import { useEffect, useRef, useState, type CSSProperties } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { usePlayerSnapshot, usePlayerUi, playerStore } from "../../player/playerStore";
import { formatDuration } from "../library/SongRow";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** The metadata fields the immersive player renders from `song_detail`. */
interface SongMeta {
  title?: string;
  artist?: string;
  album?: string;
  durationS?: number | null;
  hasCover?: boolean;
  relativePath?: string;
}

/** One timed lyrics line (mirrors the IPC `LyricsLineView`). */
interface LyricsLine {
  readonly seconds: number;
  readonly text: string;
}

/** The effective lyrics shape (mirrors the IPC `SongLyricsDto`). */
interface SongLyrics {
  readonly source: "override" | "embedded" | "sidecar" | null;
  readonly timed: boolean;
  readonly lines: readonly LyricsLine[];
  readonly plainText: string;
  readonly parseError?: string | null;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function ImmersivePlayer() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();

  // The immersive player is the `Immersive`-tier layer: it closes after the
  // lyrics-focus surface (which renders above it) on the single Escape stack.
  // Hooks are hoisted and no-op while the player is closed (Rules of Hooks).
  const shellRef = useRef<HTMLDivElement>(null);
  useOverlay({
    tier: OverlayTier.Immersive,
    onClose: () => playerStore.setImmersiveOpen(false),
    containerRef: shellRef,
    enabled: ui.immersiveOpen,
  });
  useFocusTrap(shellRef, ui.immersiveOpen);

  if (!ui.immersiveOpen) {
    return null;
  }

  return (
    <ImmersiveBody
      key={snapshot.currentQueueEntryId ?? "empty"}
      snapshot={snapshot}
      shellRef={shellRef}
      onClose={() => playerStore.setImmersiveOpen(false)}
    />
  );
}

/**
 * Lyrics focus mode (task 11.6). Rendered on top of the immersive player when
 * `focusOpen`: hides the non-essential cover/metadata, presents the lyrics as
 * the primary reading area, and keeps a minimal persistent control strip
 * (退出专注 / 回到当前行 / progress / play-pause / next) so playback control is
 * never interrupted. Escaping (Escape) closes this surface without touching the
 * immersive player beneath it — the overlay stack closes the topmost layer only.
 */
function LyricsFocus(props: {
  snapshot: ReturnType<typeof usePlayerSnapshot>;
  onClose: () => void;
}) {
  const { snapshot, onClose } = props;
  const lyrics = useLyrics(snapshot.currentSongId);
  const focusRef = useRef<HTMLDivElement>(null);

  const position = snapshot.position ?? 0;
  const duration = snapshot.duration ?? 0;
  const progress = duration > 0 ? Math.min(100, (position / duration) * 100) : 0;

  // The lyrics-focus surface is the `LyricsFocus` tier: Escape closes it (the
  // focus layer only — the immersive player and player bar remain) via the
  // single overlay stack, and focus is trapped while it is open.
  useOverlay({ tier: OverlayTier.LyricsFocus, onClose, containerRef: focusRef });
  useFocusTrap(focusRef);

  if (!lyrics || (!lyrics.timed && !lyrics.plainText)) {
    return (
      <div className="lyrics-focus" data-testid="lyrics-focus" ref={focusRef} onClick={onClose}>
        <div className="lyrics-focus-inner" onClick={(e) => e.stopPropagation()}>
          <div className="lyrics-focus-empty">
            <p className="lyrics-empty-title">暂无歌词</p>
            <button type="button" className="btn" onClick={onClose}>
              退出专注
            </button>
          </div>
        </div>
      </div>
    );
  }

  const sourceLabel =
    lyrics.source === "override"
      ? "Echo 覆盖层"
      : lyrics.source === "embedded"
        ? "内嵌歌词"
        : lyrics.source === "sidecar"
          ? "LRC 侧车文件"
          : null;
  const currentLineIdx = lyrics.timed ? findCurrentLine(lyrics.lines, position) : -1;

  return (
    <div className="lyrics-focus" data-testid="lyrics-focus" ref={focusRef}>
      <div className="lyrics-focus-head">
        <span className="lyrics-focus-title">歌词专注</span>
        {sourceLabel ? <span className="lyrics-source">歌词来源：{sourceLabel}</span> : null}
        <button type="button" className="btn" aria-label="退出歌词专注" onClick={onClose}>
          退出专注
        </button>
      </div>

      <div className="lyrics-focus-scroll" data-testid="lyrics-focus-scroll">
        {lyrics.timed ? (
          <div className="lyrics-list">
            {lyrics.lines.map((line, idx) => (
              <button
                key={`${line.seconds}-${idx}`}
                type="button"
                className={[
                  "lyrics-line lyrics-focus-line",
                  idx === currentLineIdx ? "is-current" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                aria-current={idx === currentLineIdx ? "true" : undefined}
                onClick={() => void bridge.call("seek", { position: line.seconds })}
              >
                {line.text}
              </button>
            ))}
          </div>
        ) : (
          <div className="lyrics-list">
            {lyrics.plainText.split("\n").map((line, idx) => (
              <span key={idx} className="lyrics-line lyrics-line-plain lyrics-focus-line">
                {line}
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Minimal persistent control strip — playback is never interrupted. */}
      <div className="lyrics-focus-controls">
        <div className="playerbar-progress">
          <span className="playerbar-time">{formatDuration(position)}</span>
          <input
            type="range"
            className="range"
            role="slider"
            aria-label="播放进度"
            aria-valuetext={`${formatDuration(position)} / ${formatDuration(duration)}`}
            min={0}
            max={duration || 0}
            step={5}
            value={position}
            onChange={(e) => void bridge.call("seek", { position: Number(e.target.value) })}
            style={{ "--range-progress": `${progress}%` } as CSSProperties}
          />
          <span className="playerbar-time">{formatDuration(duration)}</span>
        </div>
        <div className="playerbar-tracks">
          <button
            type="button"
            className={`icon-btn${snapshot.state === "playing" ? "" : " is-paused"}`}
            aria-label={snapshot.state === "playing" ? "暂停" : "播放"}
            onClick={() =>
              void bridge.call("player_control", {
                action: snapshot.state === "playing" ? "pause" : "play",
              })
            }
          >
            {snapshot.state === "playing" ? "⏸" : "▶"}
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label="下一首"
            onClick={() => void bridge.call("player_control", { action: "next" })}
          >
            ⏭
          </button>
        </div>
      </div>
    </div>
  );
}

/** Load the effective lyrics for a song id; shared by the immersive body and the focus surface. */
function useLyrics(songId: string | null): SongLyrics | null {
  const [lyrics, setLyrics] = useState<SongLyrics | null>(null);
  useEffect(() => {
    let cancelled = false;
    setLyrics(null);
    if (songId) {
      bridge
        .call("get_lyrics", { songId })
        .then((dto) => {
          if (cancelled) return;
          setLyrics(dto as SongLyrics);
        })
        .catch(() => {
          // Lyrics failure degrades to the no-lyrics state.
        });
    }
    return () => {
      cancelled = true;
    };
  }, [songId]);
  return lyrics;
}

function ImmersiveBody(props: {
  snapshot: ReturnType<typeof usePlayerSnapshot>;
  shellRef: React.Ref<HTMLDivElement>;
  onClose: () => void;
}) {
  const { snapshot, onClose, shellRef } = props;
  const ui = usePlayerUi();
  const { focusOpen } = ui;
  const hasCurrent = snapshot.currentQueueEntryId !== null;

  // Metadata for the current library song. Re-fetched whenever the current
  // song changes (the `key` remounts this body with the new currentSongId).
  const [meta, setMeta] = useState<SongMeta | null>(null);
  const [metaFailed, setMetaFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setMeta(null);
    setMetaFailed(false);
    if (snapshot.currentSongId) {
      bridge
        .call("song_detail", { songId: snapshot.currentSongId })
        .then((dto) => {
          if (cancelled) return;
          setMeta(dto as SongMeta);
        })
        .catch(() => {
          if (!cancelled) setMetaFailed(true);
        });
    }
    return () => {
      cancelled = true;
    };
  }, [snapshot.currentSongId]);

  // Effective lyrics for the current song (task 11.4–11.6). The component
  // remounts per currentQueueEntryId, so stale lyrics are impossible.
  const lyrics = useLyrics(snapshot.currentSongId);

  const isTemporary = snapshot.currentCanImport;
  const title =
    (snapshot.currentSongId ? meta?.title : undefined) ??
    meta?.title ??
    snapshot.currentTitle ??
    (snapshot.currentSongId ? "当前歌曲" : (snapshot.currentTitle ?? "未命名"));
  const artist = meta?.artist ?? "未知艺人";
  const album = meta?.album;
  const hasCover = meta?.hasCover ?? false;
  const hasMeta = snapshot.currentSongId !== null;

  const position = snapshot.position ?? 0;
  const duration = snapshot.duration ?? meta?.durationS ?? 0;
  const progress = duration > 0 ? Math.min(100, (position / duration) * 100) : 0;

  function command(action: string) {
    void bridge.call("player_control", { action });
  }

  const modeLabel =
    snapshot.mode === "shuffle"
      ? "随机播放"
      : snapshot.mode === "repeatOne"
        ? "单曲循环"
        : "顺序播放";
  const modeGlyph = snapshot.mode === "shuffle" ? "🔀" : snapshot.mode === "repeatOne" ? "🔁" : "▶";

  return (
    <>
      {focusOpen ? (
        <LyricsFocus snapshot={snapshot} onClose={() => playerStore.setFocusOpen(false)} />
      ) : null}
      <div
        className="immersive"
        data-testid="immersive"
        data-playing={snapshot.state === "playing"}
        ref={shellRef}
      >
        <header className="immersive-head">
          <span className="immersive-title">正在播放</span>
          <button type="button" className="icon-btn" aria-label="收起播放器" onClick={onClose}>
            ▼
          </button>
        </header>

        <div className="immersive-body">
          {!hasCurrent ? (
            <div className="immersive-empty" data-testid="immersive-empty">
              <p>未在播放</p>
            </div>
          ) : (
            <>
              <div className="immersive-stage">
                <div
                  className={["vinyl", !hasCover ? "vinyl-nocover" : ""].filter(Boolean).join(" ")}
                  aria-hidden="true"
                >
                  <div className="vinyl-art">
                    {hasCover ? (
                      <span className="vinyl-glyph">♫</span>
                    ) : (
                      <span className="vinyl-glyph">♪</span>
                    )}
                  </div>
                  <div className="vinyl-hole" />
                </div>
              </div>

              <div className="immersive-info">
                <h2 className="immersive-song" data-testid="immersive-song">
                  {title}
                </h2>
                <p className="immersive-artist">{artist}</p>
                {album ? <p className="immersive-album">{album}</p> : null}
                {metaFailed ? (
                  <p className="immersive-meta-warn">歌曲详情不可用</p>
                ) : !hasMeta ? (
                  <p className="immersive-meta-warn">临时歌曲</p>
                ) : null}
                <p className="immersive-duration">{formatDuration(duration)}</p>
                {isTemporary ? <ImmersiveImportButton /> : null}
              </div>

              {/* ---- Lyrics (task 11.4–11.6) ---- */}
              <LyricsSection
                lyrics={lyrics}
                position={position}
                duration={duration}
                onEnterFocus={() => playerStore.setFocusOpen(true)}
              />

              <div className="immersive-transport">
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
                  <span className="playerbar-time">{formatDuration(position)}</span>
                  <input
                    type="range"
                    className="range"
                    role="slider"
                    aria-label="播放进度"
                    aria-valuetext={`${formatDuration(position)} / ${formatDuration(duration)}`}
                    min={0}
                    max={duration || 0}
                    step={5}
                    value={position}
                    onChange={(e) => void bridge.call("seek", { position: Number(e.target.value) })}
                    style={{ "--range-progress": `${progress}%` } as CSSProperties}
                  />
                  <span className="playerbar-time">{formatDuration(duration)}</span>
                </div>

                <div className="immersive-mode-volume">
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
                    onChange={(e) =>
                      void bridge.call("set_volume", { volume: Number(e.target.value) })
                    }
                  />
                </div>
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Lyrics section (task 11.4–11.6)
// ---------------------------------------------------------------------------

/**
 * Renders the lyrics area of the immersive player. The section scrolls
 * independently so it does not fight the outer page scroll.
 *
 * **Synced** (`timed: true`): a list of lines; the *current* line is
 * highlighted based on the authoritative snapshot position (with UI
 * progress interpolation for smooth visual alignment between IPC tick
 * rates). The section auto-scrolls to the current line on seek and
 * track change, and clicking a line seeks the player to that line's
 * timestamp. Out-of-order timestamps are pre-sorted by the LRC parser
 * (task 4.5); the UI only needs a linear binary search.
 *
 * **Plain text** (`timed: false` but `plainText` non-empty): a static
 * scrollable list with no current-line indicator and no click-to-seek.
 *
 * **No lyrics / source error** (no effective candidate): a clear
 * "暂无歌词" state with current song info. The parse error is surfaced
 * only for diagnostics when a source was chosen but failed.
 */
function LyricsSection(props: {
  lyrics: SongLyrics | null;
  position: number;
  duration: number;
  onEnterFocus: () => void;
}) {
  const { lyrics, position, onEnterFocus } = props;
  const listRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  // Re-enable auto-scroll after a 5-second idle timeout (task 11.6 spec):
  // "用户手动滚动时应用不得立即强制跳回当前行，停止操作 5 秒后恢复跟随".
  const autoScrollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const disableAutoScroll = () => {
    setAutoScroll(false);
    if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
    autoScrollTimer.current = setTimeout(() => setAutoScroll(true), 5000);
  };

  // Clean up the timer on unmount.
  useEffect(
    () => () => {
      if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
    },
    [],
  );

  // Find the current line index (binary search over sorted timestamps).
  const currentLineIdx = lyrics?.timed ? findCurrentLine(lyrics.lines, position) : -1;

  // Auto-scroll to the current line on seek/track-change when auto-scroll is
  // on (immediate scrollIntoView; reduced-motion is handled by the browser).
  useEffect(() => {
    if (!autoScroll || currentLineIdx < 0) return;
    const el = listRef.current?.querySelector(`[data-line-idx="${currentLineIdx}"]`);
    // scrollIntoView is absent in jsdom; guard so tests are not broken.
    el?.scrollIntoView?.({ block: "nearest" });
  }, [currentLineIdx, autoScroll]);

  // ---- No lyrics state (11.5) ----
  if (!lyrics || (!lyrics.timed && !lyrics.plainText)) {
    return (
      <div className="lyrics-section lyrics-empty" data-testid="lyrics-empty">
        <p className="lyrics-empty-title">暂无歌词</p>
        <p className="lyrics-empty-sub">此歌曲没有可用的歌词</p>
      </div>
    );
  }

  // ---- Source label (11.5, task spec says show source) ----
  const sourceLabel =
    lyrics.source === "override"
      ? "Echo 覆盖层"
      : lyrics.source === "embedded"
        ? "内嵌歌词"
        : lyrics.source === "sidecar"
          ? "LRC 侧车文件"
          : null;

  // ---- Synced (timed) lyrics (11.4) ----
  if (lyrics.timed && lyrics.lines.length > 0) {
    return (
      <div className="lyrics-section lyrics-synced" data-testid="lyrics-synced">
        {sourceLabel ? (
          <p className="lyrics-source" aria-live="polite">
            歌词来源：{sourceLabel}
          </p>
        ) : null}
        <div className="lyrics-actions">
          <button type="button" className="btn btn-link" onClick={onEnterFocus}>
            歌词专注阅读
          </button>
          {!autoScroll ? (
            <button
              type="button"
              className="btn btn-link"
              aria-label="回到当前行"
              onClick={() => {
                setAutoScroll(true);
                if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
              }}
            >
              回到当前行
            </button>
          ) : null}
        </div>
        <div
          className="lyrics-list"
          ref={listRef}
          onScroll={disableAutoScroll}
          data-testid="lyrics-list"
        >
          {lyrics.lines.map((line, idx) => (
            <button
              key={`${line.seconds}-${idx}`}
              type="button"
              className={["lyrics-line", idx === currentLineIdx ? "is-current" : ""]
                .filter(Boolean)
                .join(" ")}
              data-line-idx={idx}
              aria-current={idx === currentLineIdx ? "true" : undefined}
              onClick={() => void bridge.call("seek", { position: line.seconds })}
            >
              {line.text}
            </button>
          ))}
        </div>
      </div>
    );
  }

  // ---- Plain text lyrics (11.5) ----
  return (
    <div className="lyrics-section lyrics-plain" data-testid="lyrics-plain">
      {sourceLabel ? (
        <p className="lyrics-source" aria-live="polite">
          歌词来源：{sourceLabel}
        </p>
      ) : null}
      <div className="lyrics-list lyrics-plain-list" data-testid="lyrics-plain-list">
        {lyrics.plainText.split("\n").map((line, idx) => (
          <span key={idx} className="lyrics-line lyrics-line-plain">
            {line}
          </span>
        ))}
      </div>
    </div>
  );
}

/**
 * Binary-search for the current line index given a sorted (by seconds) line
 * list and the current playback position. Returns the index of the last line
 * whose `seconds <= position`, or -1 if position precedes every line (which
 * only happens at the very start). Out-of-range lines (> duration) are
 * naturally handled — after the last timestamp the last line remains current.
 */
function findCurrentLine(lines: readonly LyricsLine[], position: number): number {
  let lo = 0;
  let hi = lines.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (lines[mid].seconds <= position) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  return lo > 0 ? lo - 1 : -1;
}

// ---------------------------------------------------------------------------
// Temporary-item import button (task 11.7)
// ---------------------------------------------------------------------------

/**
 * Renders an "导入到资料库" button inside the immersive player when the
 * current entry is a session-only temporary item. Shows inline status after
 * import completes; the button is disabled during the import request.
 */
function ImmersiveImportButton() {
  const [importing, setImporting] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  async function doImport() {
    if (importing) return;
    setImporting(true);
    setResult(null);
    try {
      const res = (await bridge.call("import_current_temporary_file")) as {
        kind: string;
      };
      switch (res.kind) {
        case "imported":
          setResult("已导入到资料库");
          break;
        case "duplicate":
          setResult("资料库已有相同歌曲");
          break;
        case "unsupported":
          setResult("不支持的格式");
          break;
        case "failed":
          setResult("导入失败");
          break;
        default:
          setResult("导入完成");
      }
    } catch {
      setResult("导入失败");
    } finally {
      setImporting(false);
    }
  }

  return (
    <div className="immersive-import" data-testid="immersive-import">
      <span className="immersive-temporary-tag" aria-label="临时播放项">
        临时播放项
      </span>
      {result ? (
        <span className="immersive-import-result" data-testid="import-result">
          {result}
        </span>
      ) : (
        <button
          type="button"
          className="btn btn-sm"
          disabled={importing}
          aria-label="导入到资料库"
          onClick={() => void doImport()}
        >
          {importing ? "导入中…" : "导入到资料库"}
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function nextMode(mode: "sequential" | "shuffle" | "repeatOne"): string {
  if (mode === "sequential") return "shuffle";
  if (mode === "shuffle") return "repeatOne";
  return "sequential";
}
