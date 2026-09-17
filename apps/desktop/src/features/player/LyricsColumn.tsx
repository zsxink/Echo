import { useLayoutEffect, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from "react";

import { bridge } from "../../bridge";
import type { LyricsLine, SongLyrics } from "./useLyrics";
import { isWideLayout } from "./useLyricsStartOffset";

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
export function LyricsColumn(props: {
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
  const onBlockClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    // `.lyric-line` seeks; 回到当前行 is a separate affordance and must not
    // double as the reading toggle either.
    if (target.closest(".lyric-line, button, a, input, select, textarea")) return;
    onToggleFocus();
  };

  // Keyboard entry (prototype 1793 handles Enter/Space on the same card).
  // Lyric-line buttons own Enter/Space themselves, so only the block's own
  // focus counts.
  const onBlockKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
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
      {/* Only while auto-follow is paused (spec: 显示可操作的"回到当前行"入口).
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
              onClick={() => bridge.fireAndForget("seek", { position: line.seconds })}
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
