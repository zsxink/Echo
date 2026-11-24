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

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type MutableRefObject,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

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
import { useOptionalCollectionNavigation } from "../library";
import { clearArtworkTint, useArtworkTint } from "./useArtworkTint";
import { useLyrics } from "./useLyrics";
import { useLyricsFollow } from "./useLyricsFollow";
import { useLyricsStartOffset } from "./useLyricsStartOffset";
import { LyricsColumn } from "./LyricsColumn";
import {
  IMMERSIVE_EXIT_MS,
  prefersReducedMotion,
  useImmersiveTransition,
  type ImmersivePhase,
} from "./useImmersiveTransition";
import { useImmersiveCoverFlip } from "./useImmersiveCoverFlip";

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function ImmersivePlayer() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();

  // Issue #85: the surface outlives `ui.immersiveOpen` by one short exit so the
  // close is continuous. `phase` is the only mount authority; the overlay stack
  // and focus trap keep reading the authoritative flag, so Escape/pointer/focus
  // semantics end the moment the user closes, not when the animation ends.
  const { phase, settled } = useImmersiveTransition(ui.immersiveOpen);

  // 歌词专注阅读 is cleared together with `immersiveOpen`, but the exiting
  // surface must not reflow into the ordinary layout first: remember whether the
  // *last stable* frame was focused and keep that layout until unmount.
  const closingFocusRef = useRef(false);
  if (phase === "closed") closingFocusRef.current = false;
  // While a close is already requested `ui.focusOpen` has been cleared in the
  // same store update, but `phase` has not caught up yet. Remember the last
  // frame in which the surface was still authoritative instead of adopting that
  // interim false.
  else if (phase !== "closing" && ui.immersiveOpen) closingFocusRef.current = ui.focusOpen;
  const focusLayout =
    phase === "closed" ? false : phase === "closing" ? closingFocusRef.current : ui.focusOpen;

  useLayoutEffect(() => {
    const body = document.body;
    body.classList.toggle("player-mode-open", phase !== "closed");
    body.classList.toggle("player-mode-entering", phase === "opening");
    body.classList.toggle("player-mode-closing", phase === "closing");
    body.classList.toggle("lyrics-focus-open", focusLayout);
    return () => {
      body.classList.remove(
        "player-mode-open",
        "player-mode-entering",
        "player-mode-closing",
        "lyrics-focus-open",
      );
    };
  }, [phase, focusLayout]);

  // Keep the cover's colour through the exit: clearing it at close time would
  // snap the surface and the 常驻播放栏 back to the theme on the first frame of
  // the fade. Cleanup happens only when the surface is actually gone.
  useLayoutEffect(() => {
    if (phase === "closed") clearArtworkTint();
  }, [phase]);
  useLayoutEffect(() => () => clearArtworkTint(), []);

  // The immersive player is the `Immersive`-tier layer. 歌词专注阅读 is *not* a
  // separate layer — it is a state of this same surface (see below) — but it
  // still unwinds first on Escape, so it registers at its own tier
  // (`… → 菜单/队列 → 歌词专注 → 沉浸式播放器 → 窄屏侧边栏`).
  // Hooks are hoisted and no-op while the player is closed (Rules of Hooks).
  const shellRef = useRef<HTMLDivElement>(null);
  useImmersiveCoverFlip(shellRef, phase);
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
    // 歌词专注阅读 is a *state* of the immersive surface, so it only counts as a
    // higher Escape layer while it is actually active. Registering it for the
    // whole lifetime of the surface made Escape in the ordinary layout close a
    // no-op focus layer first instead of the player.
    enabled: ui.immersiveOpen && ui.focusOpen,
  });
  // 收起按钮 portal 到 body（桌面平台要与窗口控件同排，必须跳出沉浸层的
  // stacking context，见 ImmersiveBody 内注释）——陷阱把这一额外区域一并圈住，
  // Tab 仍能到达它。
  const dismissSlotRef = useRef<HTMLDivElement | null>(null);
  useFocusTrap(shellRef, ui.immersiveOpen, dismissSlotRef);

  if (phase === "closed") {
    return null;
  }

  return (
    <ImmersiveBody
      snapshot={snapshot}
      shellRef={shellRef}
      dismissSlotRef={dismissSlotRef}
      phase={phase}
      geometrySettled={settled}
      focusLayout={focusLayout}
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
  dismissSlotRef: MutableRefObject<HTMLDivElement | null>;
  phase: ImmersivePhase;
  /** True once the entry transform has finished (issue #168). */
  geometrySettled: boolean;
  focusLayout: boolean;
  onClose: () => void;
}) {
  const { snapshot, onClose, shellRef, dismissSlotRef, phase, geometrySettled, focusLayout } =
    props;
  const ui = usePlayerUi();
  const hasCurrent = snapshot.currentQueueEntryId !== null;

  // 收起按钮的 portal 槽。沉浸表面（.now-playing-popover）是 z-index: 8 的
  // 独立 stacking context，而窗口拖拽条（.titlebar-drag，z-index 40）和窗口
  // 控件（45）都在它之上——桌面平台上收起按钮要落进那条 28px 标题栏带、与
  // 窗口控件同排，留在表面内部就永远压不过拖拽条。portal 到 body 后它成为
  // 顶层 chrome（45），同时把槽交给焦点陷阱，Tab 仍能到达。
  const [dismissSlot] = useState(() => document.createElement("div"));
  useLayoutEffect(() => {
    document.body.appendChild(dismissSlot);
    dismissSlotRef.current = dismissSlot;
    return () => {
      dismissSlot.remove();
      if (dismissSlotRef.current === dismissSlot) dismissSlotRef.current = null;
    };
  }, [dismissSlot, dismissSlotRef]);

  // Once the exit starts the surface is visually present but no longer an
  // interactive layer: the overlay stack already dropped it, and this makes the
  // same fact true for pointer, Tab and assistive technology. `inert` is
  // written as an attribute (React 18 does not type it); `aria-hidden` and the
  // stylesheet's `pointer-events: none` are the cross-WebView fallbacks.
  useLayoutEffect(() => {
    const surface = shellRef.current;
    if (!surface) return;
    if (phase === "closing") {
      surface.setAttribute("aria-hidden", "true");
      surface.setAttribute("inert", "");
    } else {
      surface.removeAttribute("aria-hidden");
      surface.removeAttribute("inert");
    }
  }, [phase, shellRef]);

  // Metadata for the current library song. The body no longer remounts per
  // queue entry (issue #85): `useSongDetail` / `useLyrics` reset on `songId`, so
  // the surface keeps its node, scroll container, focus state and entry
  // animation across a track change.
  const detail = useSongDetail(snapshot.currentSongId);
  // Issue #118: the overlay covers the workspace, so its links ask the shell
  // through the same optional context the list rows use.
  const navigation = useOptionalCollectionNavigation();
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
    coverKey && failedCoverId !== (songId ?? coverKey) ? assetUrl(coverKey, "original") : null;
  useArtworkTint(coverKey, artwork, !!coverKey && failedCoverId !== (songId ?? coverKey));

  // 切歌连续性: keep the previous label for one short crossfade instead of
  // blinking through the drawn placeholder. A reduced-motion switch drops it
  // immediately — the spec's exception list has no crossfade. The old image is
  // only remembered within this surface, so no stale cover can outlive the
  // queue entry it belongs to.
  const [previousArtwork, setPreviousArtwork] = useState<string | null>(null);
  const lastArtworkRef = useRef<string | null>(null);
  useEffect(() => {
    if (!artwork) {
      // A track change passes through "no artwork yet" while the new keys are
      // in flight. Hold the last label for the same short window instead of
      // blinking through the drawn placeholder; a song that really has no cover
      // lands on the placeholder when that window closes.
      if (lastArtworkRef.current && !prefersReducedMotion()) {
        setPreviousArtwork(lastArtworkRef.current);
        const timer = window.setTimeout(() => {
          setPreviousArtwork(null);
          lastArtworkRef.current = null;
        }, IMMERSIVE_EXIT_MS);
        return () => window.clearTimeout(timer);
      }
      setPreviousArtwork(null);
      lastArtworkRef.current = null;
      return;
    }
    const before = lastArtworkRef.current;
    lastArtworkRef.current = artwork;
    if (prefersReducedMotion() || !before || before === artwork) {
      setPreviousArtwork(null);
      return;
    }
    // 切歌 crossfade: the outgoing label stays painted underneath while the
    // incoming one fades in over `--motion-exit`. Only opacity animates.
    setPreviousArtwork(before);
    const timer = window.setTimeout(() => setPreviousArtwork(null), IMMERSIVE_EXIT_MS);
    return () => window.clearTimeout(timer);
  }, [artwork]);

  const duration = snapshot.duration ?? detail?.durationS ?? 0;

  // Everything that changes the *rendered height* of the metadata block: the
  // lyrics column is offset below it by measurement, not by the stylesheet.
  // 歌词专注阅读 is part of the same geometry question — that layout gives the
  // lyrics the whole surface and owns their margin itself, so the measurement
  // has to be told to stand down and re-run on the way back.
  const infoSignature = `${title}|${artist ?? ""}|${album ?? ""}|${duration}`;
  useLyricsStartOffset(shellRef, infoSignature, focusLayout);

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
      data-phase={phase}
      data-playing={snapshot.state === "playing"}
      data-focus={focusLayout}
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
      {createPortal(
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
        </button>,
        dismissSlot,
      )}

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
              {previousArtwork ? (
                <img
                  className="ring-art is-crossfading-out"
                  src={previousArtwork}
                  alt=""
                  aria-hidden="true"
                />
              ) : null}
              {artwork ? (
                <img
                  key={artwork}
                  className={`ring-art${previousArtwork ? " is-crossfading-in" : ""}`}
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
              {`歌手：`}
              {navigation && detail?.artistKey ? (
                <button
                  type="button"
                  className="song-meta-link"
                  aria-label={`打开歌手 ${artist ?? ""}`}
                  onClick={() => {
                    const key = detail.artistKey;
                    if (!key) return;
                    playerStore.setFocusOpen(false);
                    playerStore.setImmersiveOpen(false);
                    navigation.openArtist(key);
                  }}
                >
                  {artist ?? "未知艺人"}
                </button>
              ) : (
                (artist ?? "未知艺人")
              )}
              {album && detail?.albumKey && navigation ? (
                <>
                  {"\u3000专辑："}
                  <button
                    type="button"
                    className="song-meta-link"
                    aria-label={`打开专辑 ${album}`}
                    onClick={() => {
                      const key = detail.albumKey;
                      if (!key) return;
                      playerStore.setFocusOpen(false);
                      playerStore.setImmersiveOpen(false);
                      navigation.openAlbum(key);
                    }}
                  >
                    {album}
                  </button>
                </>
              ) : album ? (
                `\u3000专辑：${album}`
              ) : (
                ""
              )}
              {duration > 0 ? `\u3000${formatDuration(duration)}` : ""}
            </p>
          </div>

          <SmoothLyricsColumn
            lyrics={lyrics}
            focusOpen={focusLayout}
            autoScroll={autoScroll}
            geometrySettled={geometrySettled}
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

/** Keep frame-rate lyric synchronization out of the immersive surface render. */
function SmoothLyricsColumn(props: {
  readonly lyrics: Parameters<typeof LyricsColumn>[0]["lyrics"];
  readonly focusOpen: boolean;
  readonly autoScroll: boolean;
  readonly geometrySettled: boolean;
  readonly onResume: () => void;
  readonly onToggleFocus: () => void;
}) {
  const currentPosition = useSmoothPosition() ?? 0;
  return <LyricsColumn {...props} currentPosition={currentPosition} />;
}
