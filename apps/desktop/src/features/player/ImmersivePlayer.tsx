/**
 * 沉浸式播放器 (tasks 11.3–11.6) — the prototype's `.now-playing-popover`.
 *
 * The one decisive visual act of the product (brand §7): a large spinning vinyl
 * record with a tonearm that drops while playing, the current song's metadata,
 * and the lyrics column — all on the theme-tinted dark immersion surface, with
 * the 常驻播放栏 turning to glass beneath it (`body.player-mode-open`).
 *
 * Behaviour is unchanged from the light-weight first implementation:
 *  - reads the authoritative `PlayerSnapshot` for playback state, `song_detail`
 *    for title/artist/album/cover and `get_lyrics` for the effective lyrics;
 *  - synced lyrics highlight the current line from the snapshot position,
 *    clicking a line seeks to its timestamp, and manual scrolling pauses
 *    auto-follow for 5 seconds (回到当前行 restores it);
 *  - plain-text lyrics are a static list; no lyrics is an explicit empty state
 *    and a track change clears the previous song's lyrics (component remount);
 *  - 歌词专注阅读 is a *state of this same surface*, exactly as the prototype
 *    does it (`body.lyrics-focus-open`): the lyrics take the whole surface and
 *    the cover/meta hide, while the 常驻播放栏 below keeps playback reachable.
 *    Escape unwinds it first, without closing the immersive player. The entry
 *    is the lyrics area itself (`lyricsCard` in the prototype) — there is no
 *    separate button;
 *  - the surface carries **no** transport controls of its own: the 常驻播放栏
 *    stays live and glass-coloured beneath it (`body.player-mode-open`), which
 *    is where the prototype puts every playback control.
 *
 * State is UI-transient (`playerStore.setImmersiveOpen` / `setFocusOpen`) —
 * opening or closing either never stops, resets or starts playback.
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { assetUrl, bridge } from "../../bridge";
import { useCoverKeys } from "../../app/coverArt";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { Icon } from "../../app/Icon";
import {
  usePlayerSnapshot,
  usePlayerUi,
  useSmoothPosition,
  playerStore,
} from "../../player/playerStore";
import { formatDuration } from "../library/SongRow";
import { useSongDetail } from "./useSongDetail";
import { clearArtworkTint, useArtworkTint } from "./useArtworkTint";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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
  useLayoutEffect(() => {
    if (!ui.immersiveOpen) clearArtworkTint();
    return clearArtworkTint;
  }, [ui.immersiveOpen]);

  // The immersive player is the `Immersive`-tier layer. 歌词专注阅读 is *not* a
  // separate layer — it is a state of this same surface (see below) — but it
  // still unwinds first on Escape, so it registers at its own tier
  // (`… → 菜单/队列 → 歌词专注 → 沉浸式播放器 → 窄屏侧边栏`).
  // Hooks are hoisted and no-op while the player is closed (Rules of Hooks).
  const shellRef = useRef<HTMLDivElement>(null);
  useOverlay({
    tier: OverlayTier.Immersive,
    onClose: () => playerStore.setImmersiveOpen(false),
    containerRef: shellRef,
    enabled: ui.immersiveOpen,
  });
  useOverlay({
    tier: OverlayTier.LyricsFocus,
    onClose: () => playerStore.setFocusOpen(false),
    containerRef: shellRef,
    enabled: ui.immersiveOpen && ui.focusOpen,
  });
  useFocusTrap(shellRef, ui.immersiveOpen);

  // Two body-level switches, both taken verbatim from the prototype:
  //  - `player-mode-open` paints the dark immersion surface and turns the
  //    常驻播放栏 to glass;
  //  - `lyrics-focus-open` is 歌词专注阅读 — the lyrics take the whole surface
  //    and the cover/meta hide, while the 常驻播放栏 below keeps the minimal
  //    playback controls. One surface rearranged, not a second surface stacked.
  useEffect(() => {
    document.body.classList.toggle("player-mode-open", ui.immersiveOpen);
    document.body.classList.toggle("lyrics-focus-open", ui.immersiveOpen && ui.focusOpen);
    return () => {
      document.body.classList.remove("player-mode-open");
      document.body.classList.remove("lyrics-focus-open");
    };
  }, [ui.immersiveOpen, ui.focusOpen]);

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

/** Load the effective lyrics for a song id, for the immersive surface. */
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

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

function ImmersiveBody(props: {
  snapshot: ReturnType<typeof usePlayerSnapshot>;
  shellRef: React.RefObject<HTMLDivElement>;
  onClose: () => void;
}) {
  const { snapshot, onClose, shellRef } = props;
  const ui = usePlayerUi();
  const hasCurrent = snapshot.currentQueueEntryId !== null;

  // Metadata for the current library song. The `key` remounts this body with
  // the new currentSongId, so a track change always re-reads the detail.
  const detail = useSongDetail(snapshot.currentSongId);
  const lyrics = useLyrics(snapshot.currentSongId);

  // Lyrics follow state is lifted here so 回到当前行 floats over the lyrics (it
  // is the only affordance that has to outlive the scroll position) and the
  // focus surface can restore the same flag.
  const [autoScroll, setAutoScroll] = useState(true);
  const autoScrollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
    },
    [],
  );
  const pauseFollow = () => {
    setAutoScroll(false);
    if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
    // Spec (task 11.6): 停止操作 5 秒后恢复跟随.
    autoScrollTimer.current = setTimeout(() => setAutoScroll(true), 5000);
  };
  const resumeFollow = () => {
    setAutoScroll(true);
    if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
  };

  // "The reader scrolled" is read from the *input*, never from the scroll
  // event, because auto-follow scrolls the column itself (`scrollIntoView`) and
  // the two are indistinguishable downstream. Listening on the scroll event
  // meant 回到当前行 re-armed the instant it worked: clicking it resumed the
  // follow, the follow scrolled to the current line, that scroll paused the
  // follow again — the entry never left the screen (the browser probe caught
  // it). Wheel and touch-move only ever come from a person, and the column
  // hides its scrollbar, so there is no third way to scroll it.
  const onReaderScroll = () => pauseFollow();

  const isTemporary = snapshot.currentCanImport;
  const title =
    detail?.title ?? snapshot.currentTitle ?? (snapshot.currentSongId ? "当前歌曲" : "未命名");
  const artist = detail?.artist ?? null;
  const album = detail?.album ?? null;
  const hasCover = detail?.hasCover ?? false;
  // 内置封面 (design §115 内置优先): the artwork embedded in the file becomes the
  // record's label. A song with no embedded artwork keeps the drawn label, so
  // `hasCover` alone is not enough — the key has to resolve too.
  const songId = snapshot.currentSongId;
  const coverKeys = useCoverKeys(songId ? [songId] : []);
  const [failedCoverId, setFailedCoverId] = useState<string | null>(null);
  const coverKey = songId ? (coverKeys.get(songId) ?? null) : null;
  const artwork: string | null =
    hasCover && coverKey && failedCoverId !== songId ? assetUrl(coverKey) : null;
  useArtworkTint(
    coverKey,
    artwork,
    !!songId && failedCoverId !== songId && (!detail || (hasCover && !coverKey)),
  );
  const position = useSmoothPosition() ?? 0;
  const duration = snapshot.duration ?? detail?.durationS ?? 0;

  // Everything that changes the *rendered height* of the metadata block: the
  // lyrics column is offset below it by measurement, not by the stylesheet.
  const infoSignature = `${title}|${artist ?? ""}|${album ?? ""}|${duration}|${isTemporary}`;
  useLyricsStartOffset(shellRef, infoSignature);

  // Leaving 歌词专注阅读 through 收起 returns focus to the lyrics area — the
  // prototype does the same (`lyricsCard.focus()`,
  // echo-desktop-player.html:1790), and the lyrics area is now the entry the
  // user came back from.
  const exitFocus = () => {
    playerStore.setFocusOpen(false);
    shellRef.current?.querySelector<HTMLElement>(".lyrics")?.focus();
  };

  return (
    <div
      className="now-playing-popover"
      data-testid="immersive"
      data-playing={snapshot.state === "playing"}
      data-focus={ui.focusOpen}
      role="dialog"
      aria-labelledby="now-title"
      ref={shellRef}
      /* The scroller is not the same element in every layout — ≥981px scrolls
         `.lyrics`, 761–980px scrolls this surface itself — so the reader's
         input is caught here in the capture phase, where both are in scope.
         See `onReaderScroll` for why it is not the scroll event. */
      onWheelCapture={onReaderScroll}
      onTouchMoveCapture={onReaderScroll}
    >
      <button
        type="button"
        className="mode-dismiss"
        // The prototype's 收起 button doubles as the way out of 歌词专注阅读
        // (`setLyricsFocus` rewrites its label, `modeDismiss` closes the focus
        // state first — echo-desktop-player.html:1787/1790).
        aria-label={ui.focusOpen ? "退出歌词专注阅读" : "收起播放器"}
        onClick={ui.focusOpen ? exitFocus : onClose}
        data-testid="close-player-mode"
      >
        <Icon name="chevronDown" />
      </button>

      {!hasCurrent ? (
        <p className="now-playing-empty">未在播放</p>
      ) : (
        <>
          <div
            className={[
              "playing-cover",
              snapshot.state === "playing" ? "is-playing" : "",
              artwork ? "" : "vinyl-nocover",
            ]
              .filter(Boolean)
              .join(" ")}
            id="now-playing-cover"
            data-testid="now-playing-cover"
            aria-hidden="true"
          >
            <span className="tonearm">
              <svg viewBox="0 0 620 260" role="presentation">
                <path className="arm-path" d="M95 62L316 181C329 188 338 188 350 188H414" />
                <circle className="arm-pivot-halo" cx="95" cy="62" r="27" />
                <circle className="arm-pivot" cx="95" cy="62" r="17" />
                <circle className="arm-hub" cx="95" cy="62" r="5" />
                <g transform="translate(410 188)">
                  <rect className="cartridge" x="0" y="-14" width="86" height="28" rx="6" />
                  <rect className="cartridge-front" x="81" y="-19" width="34" height="38" rx="7" />
                  <path className="cartridge-detail" d="M90 -6H106M90 6H106" />
                </g>
              </svg>
            </span>
            <span className="ring">
              {artwork ? (
                <img
                  className="ring-art"
                  src={artwork}
                  alt=""
                  onError={() => setFailedCoverId(songId)}
                />
              ) : null}
            </span>
          </div>

          <div className="now-playing-info" data-testid="now-playing-info">
            <h3 className="now-title" id="now-title" data-testid="immersive-song">
              {title}
            </h3>
            <p className="song-meta">
              {/* 原型用全角空格 U+3000 分隔「歌手 / 专辑 / 时长」，
                    这里写成转义，避免 no-irregular-whitespace，字符保持一致。 */}
              {`歌手：${artist ?? "未知艺人"}`}
              {album ? `\u3000专辑：${album}` : ""}
              {duration > 0 ? `\u3000${formatDuration(duration)}` : ""}
            </p>
            {isTemporary ? <ImmersiveImport /> : null}
          </div>

          <LyricsColumn
            lyrics={lyrics}
            focusOpen={ui.focusOpen}
            autoScroll={autoScroll}
            currentPosition={position}
            onResume={resumeFollow}
            onToggleFocus={() => playerStore.setFocusOpen(!ui.focusOpen)}
          />
        </>
      )}

      {/* No transport strip inside the surface: the prototype keeps every
            playback control in the 常驻播放栏, which stays live (and turns to
            glass) beneath the immersive surface. Repeating 上一首/播放/下一首/模式
            up here duplicated the bar and ate the lyrics column. */}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Lyrics column (task 11.4–11.6)
// ---------------------------------------------------------------------------

/**
 * The lyrics area of the immersive surface — the prototype's `.lyrics` block:
 * a scrolling lyrics column, nothing above it.
 *
 * There is deliberately **no** header row. The prototype ships a `.lyrics-label`
 * rule but never renders the element, and the product drops the source label
 * outright: the lyrics own the whole block. 回到当前行 is the one exception —
 * it only exists while auto-follow is paused, so it floats *over* the column
 * (absolutely positioned against the surface, hence immune to the column's own
 * scrolling) instead of pushing the lyrics down a line for the 99% of the time
 * it is not needed.
 *
 * **Synced** (`timed: true`): the current line is highlighted from the
 * authoritative position and clicks seek to that line. Auto-follow pauses on
 * manual scroll and resumes 5 seconds later (or immediately via 回到当前行).
 * **Plain text**: a static list, no current line, no seek.
 * **No lyrics**: an explicit `.lyrics-empty` state, never an empty box.
 *
 * 歌词专注阅读 (`focusOpen`) is a state of this same block, not a second
 * surface and not a button: the prototype makes the whole lyrics card the
 * toggle (`lyricsCard` → `setLyricsFocus`, echo-desktop-player.html:1792/1793),
 * so activating the lyrics area — pointer or keyboard — flips it, and the 收起
 * entry in the immersive surface leaves it again.
 */
function LyricsColumn(props: {
  lyrics: SongLyrics | null;
  focusOpen: boolean;
  autoScroll: boolean;
  currentPosition: number;
  onResume: () => void;
  onToggleFocus: () => void;
}) {
  const { lyrics, focusOpen, autoScroll, currentPosition, onResume, onToggleFocus } = props;
  const trackRef = useRef<HTMLDivElement>(null);

  const currentLineIdx = lyrics?.timed ? findCurrentLine(lyrics.lines, currentPosition) : -1;

  // Align the active line with the record's centre, including at either end
  // of the song. Focus and stacked layouts use the lyrics viewport centre.
  useLayoutEffect(() => {
    const track = trackRef.current;
    const viewport = track?.closest<HTMLElement>(".lyrics");
    const shell = track?.closest<HTMLElement>(".now-playing-popover");
    if (!track || !viewport || !shell || !autoScroll || currentLineIdx < 0) return;

    const align = () => {
      const current = track.querySelector<HTMLElement>(`[data-line-idx="${currentLineIdx}"]`);
      if (!current || !viewport.clientHeight) return;
      const bounds = viewport.getBoundingClientRect();
      const cover = shell.querySelector<HTMLElement>(".playing-cover")?.getBoundingClientRect();
      const targetY =
        !focusOpen && isWideLayout() && cover?.height
          ? cover.top + cover.height / 2
          : bounds.top + viewport.clientHeight / 2;
      const targetOffset = targetY - bounds.top;
      const style = getComputedStyle(viewport);
      const first = track.firstElementChild?.getBoundingClientRect();
      const last = track.lastElementChild?.getBoundingClientRect();
      track.style.setProperty(
        "--lyrics-edge-top",
        `${Math.max(0, targetOffset - parseFloat(style.paddingTop) - (first?.height ?? 0) / 2)}px`,
      );
      track.style.setProperty(
        "--lyrics-edge-bottom",
        `${Math.max(
          0,
          viewport.clientHeight -
            targetOffset -
            parseFloat(style.paddingBottom) -
            (last?.height ?? 0) / 2,
        )}px`,
      );
      const line = current.getBoundingClientRect();
      viewport.scrollTop += line.top + line.height / 2 - targetY;
    };
    // Run after the parent's metadata offset has been applied.
    let frame = requestAnimationFrame(align);
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(align);
    };
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    observer?.observe(viewport);
    observer?.observe(track);
    observer?.observe(shell);
    window.addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
      window.removeEventListener("resize", schedule);
    };
  }, [currentLineIdx, autoScroll, focusOpen, lyrics]);

  if (!lyrics || (!lyrics.timed && !lyrics.plainText)) {
    return (
      <div className="lyrics lyrics-empty" data-testid="lyrics-empty">
        <p>暂无歌词</p>
      </div>
    );
  }

  // The prototype's `lyricsCard` is a <button>, so the whole card toggles. Echo
  // cannot copy that: the lyric lines are buttons themselves (they seek), and a
  // button inside a button is invalid. The block therefore toggles on every
  // activation that misses an interactive child, and stays reachable itself.
  const onBlockClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    // `.lyric-line` seeks; 回到当前行 is a separate affordance and must not
    // double as the reading toggle either.
    if (target.closest(".lyric-line, button, a, input, select, textarea")) return;
    onToggleFocus();
  };

  // Keyboard entry (prototype 1793 handles Enter/Space on the same card).
  // Lyric-line buttons own Enter/Space themselves, so only the block's own
  // focus counts.
  const onBlockKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    onToggleFocus();
  };

  return (
    <div
      className={`lyrics${focusOpen ? " is-focus" : ""}`}
      data-testid="lyrics"
      // Named like the prototype's card so the reading toggle is announced
      // without ever being labelled a button it is not.
      aria-label={focusOpen ? "退出歌词专注阅读" : "展开歌词专注阅读"}
      tabIndex={0}
      onClick={onBlockClick}
      onKeyDown={onBlockKeyDown}
    >
      {/* Only while auto-follow is paused (spec: 显示可操作的“回到当前行”入口).
          It floats over the column rather than claiming a row, so the lyrics
          get the whole block back the moment the reader is in sync again. */}
      {!autoScroll ? (
        <button type="button" className="lyrics-resume" onClick={onResume}>
          回到当前行
        </button>
      ) : null}

      {lyrics.timed ? (
        <div
          className="lyrics-track"
          ref={trackRef}
          data-testid="lyrics-list"
          tabIndex={0}
          aria-label="歌词"
        >
          {lyrics.lines.map((line, idx) => (
            <button
              key={`${line.seconds}-${idx}`}
              type="button"
              className={["lyric-line", idx === currentLineIdx ? "is-current" : ""]
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
      ) : (
        <div className="lyrics-track" data-testid="lyrics-plain-list">
          {lyrics.plainText.split("\n").map((line, idx) => (
            <span key={idx} className="lyric-line lyrics-line-plain">
              {line}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Temporary-item import button (task 11.7)
// ---------------------------------------------------------------------------

/**
 * "导入到资料库" inside the immersive surface when the current entry is a
 * session-only temporary item. Shows inline status after the import completes
 * and is disabled while the request is in flight.
 */
function ImmersiveImport() {
  const [importing, setImporting] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  async function doImport() {
    if (importing) return;
    setImporting(true);
    setResult(null);
    try {
      const res = (await bridge.call("import_current_temporary_file")) as { kind: string };
      setResult(importResultText(res.kind));
    } catch {
      setResult("导入失败");
    } finally {
      setImporting(false);
    }
  }

  return (
    <div className="now-playing-import" data-testid="immersive-import">
      <span className="player-temporary-tag" aria-label="临时播放项">
        临时
      </span>
      {result ? (
        <span className="now-playing-import-result" data-testid="import-result">
          {result}
        </span>
      ) : (
        <button
          type="button"
          className="btn"
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

function importResultText(kind: string): string {
  switch (kind) {
    case "imported":
      return "已导入到资料库";
    case "duplicate":
      return "资料库已有相同歌曲";
    case "unsupported":
      return "不支持的格式";
    case "failed":
      return "导入失败";
    default:
      return "导入完成";
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** The breakpoint at which `player.css` puts the meta and the lyrics in one area. */
const WIDE_LAYOUT_QUERY = "(min-width: 981px)";

/** Breathing room between the metadata block and the first lyric line. */
const LYRICS_GAP_PX = 20;

/**
 * Prototype parity for the wide layout: `@media (min-width: 981px)` gives
 * `.now-playing-info` and `.lyrics` the **same** grid area (`"record lyrics"`,
 * see `player.css`), so the lyrics only stop covering the song title because
 * the prototype measures the metadata block and writes `--lyrics-start-offset`
 * (`alignDesktopLyricsRegion`, echo-desktop-player.html:1758-1763). The offset
 * cannot live in the stylesheet: it depends on the rendered height of the
 * metadata — title wrapping, artist/album line, 临时 import row.
 *
 * Without the measurement the custom property falls back to `0px`, and the
 * faded lyric lines paint *underneath* the title (`.now-playing-info` is
 * `z-index: 1; pointer-events: none`), which reads as translucent lyrics
 * behind the song name. jsdom has no layout, so both halves of that are
 * asserted on rendered geometry in `e2e/run-e2e.mjs` instead.
 *
 * `infoSignature` carries every input that changes the metadata's height; the
 * listener covers width changes (which rewrap the title).
 */
function useLyricsStartOffset(shellRef: React.RefObject<HTMLDivElement>, infoSignature: string) {
  useLayoutEffect(() => {
    const shell = shellRef.current;
    const apply = () => {
      const info = shell?.querySelector<HTMLElement>(".now-playing-info");
      const lyrics = shell?.querySelector<HTMLElement>(".lyrics");
      if (!info || !lyrics) return;
      // Measure from the un-offset state: the property moves the very box it is
      // measured against, so it has to come off first (prototype does the same).
      lyrics.style.removeProperty("--lyrics-start-offset");
      // ≤980px stacks record / info / lyrics in one column, so there is no
      // shared grid area to separate.
      if (!isWideLayout()) return;
      const offset = Math.max(
        0,
        info.getBoundingClientRect().bottom - lyrics.getBoundingClientRect().top + LYRICS_GAP_PX,
      );
      lyrics.style.setProperty("--lyrics-start-offset", `${Math.round(offset)}px`);
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [shellRef, infoSignature]);
}

/**
 * jsdom implements no `matchMedia` at all (the unit suite would throw), and it
 * could not answer a width query anyway — it has no layout. The desktop WebView
 * always has it. "Not wide" is the safe answer wherever the query is missing:
 * the offset is a measurement, and nothing here can measure.
 */
function isWideLayout(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(WIDE_LAYOUT_QUERY).matches;
}

/**
 * Binary-search for the current line index given a sorted (by seconds) line
 * list and the current playback position. Returns the index of the last line
 * whose `seconds <= position`, or -1 if position precedes every line. The LRC
 * parser (task 4.5) pre-sorts timestamps, so the UI only needs a linear search.
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
