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
 *
 * Decomposition (task 6.6): the lyric data fetch (`useLyrics`), the auto-follow
 * timer (`useLyricsFollow`), the layout measurement (`useLyricsStartOffset`),
 * the lyrics column (`LyricsColumn`) and the temporary-item import
 * (`ImmersiveImport`) each live in their own module so this file keeps only the
 * overlay/body-effect orchestration and the surface layout (CODE_STANDARDS §6).
 */

import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

import { assetUrl } from "../../bridge";
import { useCoverKeys } from "../../app/coverArt";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { Icon } from "../../app/Icon";
import {
  usePlayerSnapshot,
  usePlayerUi,
  useSmoothPosition,
  playerStore,
} from "../../player/playerStore";
import { formatDuration } from "../library";
import { useSongDetail } from "./useSongDetail";
import { clearArtworkTint, useArtworkTint } from "./useArtworkTint";
import { useLyrics } from "./useLyrics";
import { useLyricsFollow } from "./useLyricsFollow";
import { useLyricsStartOffset } from "./useLyricsStartOffset";
import { LyricsColumn } from "./LyricsColumn";

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

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

function ImmersiveBody(props: {
  snapshot: ReturnType<typeof usePlayerSnapshot>;
  shellRef: RefObject<HTMLDivElement>;
  onClose: () => void;
}) {
  const { snapshot, onClose, shellRef } = props;
  const ui = usePlayerUi();
  const hasCurrent = snapshot.currentQueueEntryId !== null;

  // Metadata for the current library song. The `key` remounts this body with
  // the new currentSongId, so a track change always re-reads the detail.
  const detail = useSongDetail(snapshot.currentSongId);
  const libraryLyrics = useLyrics(snapshot.currentSongId);
  const lyrics = snapshot.currentSongId ? libraryLyrics : (snapshot.currentLyrics ?? null);

  // Lyrics follow state is lifted here so 回到当前行 floats over the lyrics (it
  // is the only affordance that has to outlive the scroll position) and the
  // focus surface can restore the same flag.
  const { autoScroll, pauseFollow, resumeFollow } = useLyricsFollow();

  // "The reader scrolled" is read from the *input*, never from the scroll
  // event, because auto-follow scrolls the column itself (`scrollIntoView`) and
  // the two are indistinguishable downstream. Listening on the scroll event
  // meant 回到当前行 re-armed the instant it worked: clicking it resumed the
  // follow, the follow scrolled to the current line, that scroll paused the
  // follow again — the entry never left the screen (the browser probe caught
  // it). Wheel and touch-move only ever come from a person, and the column
  // hides its scrollbar, so there is no third way to scroll it.
  const onReaderScroll = () => pauseFollow();

  const title =
    detail?.title ?? snapshot.currentTitle ?? (snapshot.currentSongId ? "当前歌曲" : "未命名");
  const artist = detail?.artist ?? snapshot.currentArtist ?? null;
  const album = detail?.album ?? snapshot.currentAlbum ?? null;
  // 内置封面 (design §115 内置优先): the artwork embedded in the file becomes the
  // record's label. A song with no embedded artwork keeps the drawn label, so
  // `hasCover` alone is not enough — the key has to resolve too.
  const songId = snapshot.currentSongId;
  const coverKeys = useCoverKeys(songId ? [songId] : []);
  const [failedCoverId, setFailedCoverId] = useState<string | null>(null);
  const coverKey = songId ? (coverKeys.get(songId) ?? null) : (snapshot.currentCoverKey ?? null);
  const artwork: string | null =
    coverKey && failedCoverId !== (songId ?? coverKey) ? assetUrl(coverKey) : null;
  useArtworkTint(
    coverKey,
    artwork,
    !!coverKey && failedCoverId !== (songId ?? coverKey),
  );
  const position = useSmoothPosition() ?? 0;
  const duration = snapshot.duration ?? detail?.durationS ?? 0;

  // Everything that changes the *rendered height* of the metadata block: the
  // lyrics column is offset below it by measurement, not by the stylesheet.
  const infoSignature = `${title}|${artist ?? ""}|${album ?? ""}|${duration}`;
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
                  onError={() => setFailedCoverId(songId ?? coverKey)}
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
