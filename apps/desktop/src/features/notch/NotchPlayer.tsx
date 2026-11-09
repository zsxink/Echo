/**
 * The macOS notch player window's only view (openspec `macos-notch-player`).
 *
 * It is a **third view of one playback session**, never a second player: every
 * value comes from the authoritative `PlayerSnapshot` and every control calls
 * the same commands the player bar uses (`player_control` / `seek` /
 * `set_favorite`). Visual structure and class names come from
 * `docs/prototype/echo-macos-notch-player.html` via the generated `notch.css`,
 * so the shipped surface cannot drift from the reviewed prototype.
 *
 * Window sizing is part of the interaction: the companion window is only as big
 * as the collapsed bar, and it grows *before* the panel animates (and shrinks
 * *after* it collapses) so the CSS transition is never clipped by the window
 * frame. The renderer therefore awaits `notch_player_set_expanded` instead of
 * animating first.
 *
 * Keyboard: a non-activating window cannot become key, so this surface is
 * pointer-only by design. It must never be the only way to reach a control —
 * the player bar, the menu-bar status row and the system media keys remain the
 * keyboard/AT paths (see the spec's "不得成为唯一入口" requirement).
 */

import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";

import { assetUrl, bridge } from "../../bridge";
import { useCoverKeys } from "../../app/coverArt";
import { Icon } from "../../app/Icon";
import { usePlayerSnapshot, useSmoothPosition } from "../../player/playerStore";
import { formatDuration, setSongFavorite } from "../library";
import { modeFor, useSongDetail } from "../player";

/** React 18's intrinsic-element typings predate the `inert` attribute. Spread it
 * as a plain attribute so the browser's native inert semantics apply: the
 * collapsed panel must not be reachable, and the ears must not be while the
 * panel is open. */
function inertProps(inert: boolean): Record<string, string> {
  return inert ? { inert: "" } : {};
}

/** Collapse starts after this idle beat, so a pointer brushing past the bar
 * does not flap the panel open and shut. */
const COLLAPSE_DELAY_MS = 90;
/** The collapse transition in `notch.css` is 380 ms; shrink the window only
 * after it finishes, otherwise the panel would be clipped mid-animation. */
const COLLAPSE_ANIMATION_MS = 420;

export function NotchPlayer() {
  const snapshot = usePlayerSnapshot();
  const position = useSmoothPosition() ?? 0;
  const songId = snapshot.currentSongId;
  const detail = useSongDetail(songId);
  const coverKeys = useCoverKeys(songId ? [songId] : []);

  const hasCurrent = snapshot.currentQueueEntryId !== null;
  const isTemporary = hasCurrent && snapshot.currentCanImport;
  const isPlaying = snapshot.state === "playing" && hasCurrent;

  const coverKey = songId ? (coverKeys.get(songId) ?? null) : (snapshot.currentCoverKey ?? null);
  const [failedCover, setFailedCover] = useState<string | null>(null);
  const artwork = coverKey && failedCover !== coverKey ? assetUrl(coverKey, "list") : null;

  const duration = snapshot.duration ?? detail?.durationS ?? 0;
  const title = isTemporary ? snapshot.currentTitle : (detail?.title ?? snapshot.currentTitle);
  const artist =
    detail?.artist ?? snapshot.currentArtist ?? (duration > 0 ? formatDuration(duration) : null);

  const mode = modeFor(snapshot.mode);

  // The housing width drives both the drawn bezel and (in Rust) the collapsed
  // window width, so the cover and the bars always sit clear of the bezel on
  // every MacBook size. It is a runtime value, never a stylesheet constant.
  const [notchWidth, setNotchWidth] = useState<number | null>(null);
  useEffect(() => {
    let current = true;
    void bridge
      .call("get_notch_player_state")
      .then((state) => {
        if (current) setNotchWidth(state.notchWidth);
      })
      .catch(() => {
        // Without the value the stylesheet's fallback keeps the prototype's
        // proportions; the window itself is still sized by Rust.
      });
    return () => {
      current = false;
    };
  }, []);

  // --- window growth protocol -------------------------------------------------
  const [expanded, setExpanded] = useState(false);
  const expandedRef = useRef(false);
  const draggingRef = useRef(false);
  const collapseTimer = useRef<number | null>(null);
  const shrinkTimer = useRef<number | null>(null);

  const clearTimers = useCallback(() => {
    if (collapseTimer.current !== null) {
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
    }
    if (shrinkTimer.current !== null) {
      window.clearTimeout(shrinkTimer.current);
      shrinkTimer.current = null;
    }
  }, []);

  const expand = useCallback(async () => {
    clearTimers();
    if (expandedRef.current) return;
    // Claim the state before the round trip so a second enter cannot queue a
    // duplicate resize while the first one is in flight.
    expandedRef.current = true;
    try {
      await bridge.call("notch_player_set_expanded", { expanded: true });
      setExpanded(true);
    } catch {
      // The window refused (closed, or the preference turned off): stay
      // collapsed rather than showing a panel the window cannot host.
      expandedRef.current = false;
    }
  }, [clearTimers]);

  const collapse = useCallback(() => {
    if (draggingRef.current || !expandedRef.current) return;
    if (collapseTimer.current !== null) window.clearTimeout(collapseTimer.current);
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = null;
      expandedRef.current = false;
      setExpanded(false);
      shrinkTimer.current = window.setTimeout(() => {
        shrinkTimer.current = null;
        void bridge.fireAndForget("notch_player_set_expanded", { expanded: false });
      }, COLLAPSE_ANIMATION_MS);
    }, COLLAPSE_DELAY_MS);
  }, []);

  useEffect(() => clearTimers, [clearTimers]);

  // A pointer released outside the window must not leave the panel pinned.
  useEffect(() => {
    const release = () => {
      draggingRef.current = false;
    };
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
    };
  }, []);

  function command(action: string) {
    void bridge.fireAndForget("player_control", { action });
  }

  const progress = duration > 0 ? Math.max(0, Math.min(1, position / duration)) : 0;
  const favorite = detail?.favorite ?? false;
  const favoriteDisabled = !songId || detail === null;

  return (
    <div
      className="notch-player"
      data-expanded={expanded}
      data-playing={isPlaying}
      data-empty={!hasCurrent}
      data-temporary={isTemporary}
      role="group"
      aria-label="Echo 刘海播放器"
      style={
        notchWidth === null
          ? undefined
          : ({ "--notch-hw-width": `${notchWidth}px` } as CSSProperties)
      }
      onPointerLeave={collapse}
      data-testid="notch-player"
    >
      <div className="notch-surface">
        <div className="ears" aria-hidden="true" {...inertProps(expanded)}>
          {/* Only the two ears are hover targets: the hardware housing between
              them is not part of this window's interactive area. */}
          <div
            className="ear ear-left"
            onPointerEnter={() => void expand()}
            data-testid="notch-ear-left"
          >
            <span className={`thumb${artwork ? " has-image" : ""}`}>
              {artwork ? (
                <img src={artwork} alt="" onError={() => setFailedCover(coverKey)} />
              ) : null}
            </span>
          </div>
          <div
            className="ear ear-right"
            onPointerEnter={() => void expand()}
            data-testid="notch-ear-right"
          >
            <span className="playing-bars compact">
              <i />
              <i />
              <i />
            </span>
          </div>
        </div>

        <div className="panel" {...inertProps(!expanded)}>
          <div className="panel-head">
            <span className={`panel-cover${artwork ? " has-image" : ""}`}>
              {artwork ? (
                <img src={artwork} alt="" onError={() => setFailedCover(coverKey)} />
              ) : null}
            </span>
            <div className="panel-meta">
              <b className={`panel-title${title ? "" : " is-empty"}`}>{title ?? "—"}</b>
              <span className="panel-sub">{artist ?? "未在播放"}</span>
            </div>
            <span className="panel-tag">临时</span>
            <div className="panel-actions">
              <div className="panel-bars">
                <span className="playing-bars">
                  <i />
                  <i />
                  <i />
                </span>
              </div>
              <button
                className="notch-collapse"
                type="button"
                aria-label="收起刘海播放器"
                onClick={collapse}
              >
                <Icon name="chevronUp" />
              </button>
            </div>
          </div>

          <div className="panel-progress">
            <div className="progress-line">
              <span
                className="progress-fill"
                aria-hidden="true"
                style={{ transform: `scaleX(${progress})` }}
              />
              <input
                className="progress"
                type="range"
                role="slider"
                aria-label="播放进度"
                aria-valuetext={`${formatDuration(position)} / ${formatDuration(duration)}`}
                min={0}
                max={duration || 0}
                step="any"
                value={Math.min(position, duration || 0)}
                disabled={!hasCurrent || duration <= 0}
                onPointerDown={() => {
                  draggingRef.current = true;
                }}
                onChange={(event) =>
                  void bridge.fireAndForget("seek", { position: Number(event.target.value) })
                }
              />
            </div>
            <div className="progress-times">
              <span>{formatDuration(position)}</span>
              <span>{formatDuration(duration)}</span>
            </div>
          </div>

          <div className="panel-controls">
            <button
              className={`notch-control${favorite ? " is-favorite" : ""}`}
              type="button"
              aria-label={favorite ? "取消喜欢当前歌曲" : "喜欢当前歌曲"}
              aria-pressed={favorite}
              disabled={favoriteDisabled}
              onClick={() => {
                if (!songId || detail === null) return;
                void setSongFavorite({ id: songId, favorite }, !favorite);
              }}
              data-testid="notch-favorite"
            >
              <Icon name="heart" filled={favorite} />
            </button>
            <button
              className="notch-control"
              type="button"
              aria-label="上一首"
              disabled={!hasCurrent}
              onClick={() => command("previous")}
            >
              <Icon name="previous" />
            </button>
            <button
              className="notch-control control-play"
              type="button"
              aria-label={isPlaying ? "暂停" : "播放"}
              disabled={!hasCurrent}
              onClick={() => command(isPlaying ? "pause" : "play")}
              data-testid="notch-play"
            >
              <Icon name={isPlaying ? "pause" : "play"} />
            </button>
            <button
              className="notch-control"
              type="button"
              aria-label="下一首"
              disabled={!hasCurrent}
              onClick={() => command("next")}
            >
              <Icon name="next" />
            </button>
            <button
              className="notch-control"
              type="button"
              id="btn-mode"
              aria-label={mode.label}
              title={mode.label}
              aria-pressed={snapshot.mode !== "sequential"}
              data-mode={snapshot.mode}
              disabled={!hasCurrent}
              onClick={() => command(`mode:${mode.next}`)}
              data-testid="notch-mode"
            >
              <Icon name={mode.icon} />
            </button>
          </div>
        </div>
      </div>
      <div className="hardware-notch" aria-hidden="true" />
    </div>
  );
}
