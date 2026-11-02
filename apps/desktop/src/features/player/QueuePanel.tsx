/**
 * 播放队列面板 (task 11.2) — the prototype's `.queue-popover`.
 *
 * Renders the authoritative queue projection: the current entry followed by
 * the pending entries, each with a cover block, title + secondary line and
 * duration, and per-entry failed / blocked state surfaced. The header carries
 * the queue size and 清空待播 (clear pending).
 *
 * The queue projection is the single source of truth — this panel never
 * fabricates a queue. Clearing pending removes only *pending* entries: the
 * current song keeps playing and neither the library nor playlists are
 * touched. Blocked (unavailable/missing) and failed entries stay visible so
 * the user understands why an item did not play.
 *
 * The list is windowed: only the rows inside the visible area (plus a small
 * margin) exist in the DOM, and the space of the rows above and below them is
 * held by the scroll padding. A 5000-entry queue therefore costs a handful of
 * rows, not 5000 (DP-R03).
 */

import { useLayoutEffect, useRef, useState, type RefObject } from "react";

import { assetUrl, bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { usePlayerQueue, usePlayerUi, playerStore } from "../../player/playerStore";
import { coverClass, formatDuration } from "../library";

/** The design baseline for one queue row (`--queue-row-height`). */
const ROW_HEIGHT = 44;
/** The panel's initial visible area holds at most this many rows. */
const VISIBLE_ROWS = 8;
/** The rows kept above and below the visible area, so a scroll does not
 *  immediately expose an empty edge. */
const OVERSCAN = 4;

function QueueCover({
  coverKey,
  identity,
}: {
  readonly coverKey: string | null;
  readonly identity: string;
}) {
  const [failed, setFailed] = useState(false);
  if (!coverKey || failed) {
    return <span className={`queue-cover ${coverClass(identity)}`} aria-label="无封面" />;
  }
  return (
    <img
      className="queue-cover"
      src={assetUrl(coverKey, "list")}
      alt=""
      data-testid="queue-cover-image"
      onError={() => setFailed(true)}
    />
  );
}

export function QueuePanel({
  triggerRef,
}: {
  readonly triggerRef?: RefObject<HTMLElement | null>;
}) {
  const queue = usePlayerQueue();
  const ui = usePlayerUi();
  const panelRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  // The measured row pitch and visible row count. The CSS owns the sizes, so
  // the window is measured from the rendered list instead of being guessed
  // from a constant that could drift away from the stylesheet.
  const [metrics, setMetrics] = useState({ pitch: ROW_HEIGHT, rows: VISIBLE_ROWS });
  // Queue panel is a `Menu`-tier overlay: Escape closes it via the single stack,
  // focus is trapped, and focus returns to the open control. Hooks are hoisted
  // above the early return (Rules of Hooks) and no-op while closed.
  //
  // `triggerRef` is the player bar's queue button, which is *not* a descendant of
  // the popover (moving the popover inside the bar would make `backdrop-filter`/
  // `transform` on `.playerbar` its containing block). Registering it keeps a
  // second press on that button from reading as an outside press — which closed
  // the panel only for the same gesture's `click` to re-open it.
  const close = () => playerStore.setQueueOpen(false);
  useOverlay({
    tier: OverlayTier.Menu,
    onClose: close,
    containerRef: panelRef,
    triggerRef,
    enabled: ui.queueOpen,
  });
  useFocusTrap(panelRef, ui.queueOpen);

  const entries = queue.entries;

  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const row = list.querySelector<HTMLElement>(".queue-item");
    const gap = parseFloat(getComputedStyle(list.firstElementChild as Element).rowGap) || 0;
    const height = row?.getBoundingClientRect().height ?? 0;
    const pitch = height > 0 ? height + gap : ROW_HEIGHT;
    const rows = list.clientHeight > 0 ? Math.max(1, Math.ceil(list.clientHeight / pitch)) : VISIBLE_ROWS;
    setMetrics((current) =>
      current.pitch === pitch && current.rows === rows ? current : { pitch, rows },
    );
  }, [entries.length, ui.queueOpen]);

  if (!ui.queueOpen) {
    return null;
  }

  const hasPending = entries.some((entry) => !entry.isCurrent);
  // The window of rows that exist in the DOM: the visible area plus the
  // margin. Its bounds never depend on the queue length.
  const first = Math.max(0, Math.floor(scrollTop / metrics.pitch) - OVERSCAN);
  const last = Math.min(
    entries.length,
    Math.ceil((scrollTop + metrics.rows * metrics.pitch) / metrics.pitch) + OVERSCAN,
  );
  const visible = entries.slice(first, last);

  function command(action: string) {
    bridge.fireAndForget("queue_command", { command: action });
  }

  return (
    <section
      className="queue-popover"
      data-testid="queue-panel"
      role="dialog"
      aria-label="播放队列"
      ref={panelRef}
    >
      <div className="queue">
        <div className="queue-title">
          <div className="queue-heading">
            <h3>播放列表</h3>
            <span id="queue-count">{entries.length} 首</span>
          </div>
          <div className="queue-tools">
            {entries.some((entry) => entry.blocked) ? (
              <button type="button" className="queue-tool" onClick={() => command("retryBlocked")}>
                重试不可用项
              </button>
            ) : null}
            <button
              type="button"
              className="queue-tool"
              disabled={!hasPending}
              onClick={() => command("clearPending")}
              data-testid="clear-queue"
            >
              清空待播
            </button>
          </div>
        </div>

        {entries.length === 0 ? (
          <div className="queue-empty show" data-testid="queue-empty">
            <p>队列为空</p>
            <button type="button" className="btn" onClick={close}>
              浏览曲库
            </button>
          </div>
        ) : (
          <div
            className="queue-list"
            data-testid="queue-list"
            ref={listRef}
            onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          >
            <div
              className="queue-window"
              style={{
                paddingTop: first * metrics.pitch,
                paddingBottom: (entries.length - last) * metrics.pitch,
              }}
            >
              {visible.map((entry) => (
                <button
                  type="button"
                  key={entry.entryId}
                  className={[
                    "queue-item",
                    entry.isCurrent ? "is-current" : "",
                    entry.failed ? "is-failed" : "",
                    entry.blocked ? "is-blocked" : "",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  aria-current={entry.isCurrent ? "true" : undefined}
                  aria-label={`播放 ${entry.title ?? "未知歌曲"}`}
                  disabled={entry.blocked || entry.failed}
                  onClick={() =>
                    bridge.fireAndForget("queue_command", {
                      command: "playEntry",
                      entryId: entry.entryId,
                    })
                  }
                >
                  <QueueCover coverKey={entry.coverKey} identity={entry.songId ?? entry.entryId} />
                  <div className="queue-name">
                    <b>{entry.title ?? "未知歌曲"}</b>
                    <span>
                      {entry.blocked
                        ? "暂时不可用，可在音乐库恢复后重试"
                        : entry.failed
                          ? "加载失败"
                          : (entry.artist ?? "未知艺人")}
                    </span>
                  </div>
                  <span className="queue-time">
                    {entry.durationS === null ? "时长未知" : formatDuration(entry.durationS)}
                  </span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
