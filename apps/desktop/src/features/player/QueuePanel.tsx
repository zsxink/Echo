/**
 * Playback queue panel (task 11.2).
 *
 * Renders the authoritative queue from the PlayerSnapshot (`snapshot.queue`):
 * the current entry followed by the pending entries, with per-entry failed /
 * blocked state surfaced. It offers "清空待播" (clear pending), and a browse
 * library action for the empty state.
 *
 * The snapshot is the single source of truth — this panel never fabricates a
 * queue. Clearing pending only removes *pending* entries; the current song
 * keeps playing and neither the library nor playlists are touched (the command
 * is `clearPending` on the Rust coordinator). Blocked (unavailable/missing)
 * and failed entries remain visible so the user understands why an item did
 * not play, and are skipped by the coordinator's error rules.
 */

import { useRef } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { usePlayerSnapshot, usePlayerUi, playerStore } from "../../player/playerStore";

export function QueuePanel() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();
  const panelRef = useRef<HTMLDivElement>(null);
  // Queue panel is a `Menu`/`Queue`-tier overlay: Escape closes it via the
  // single stack, focus is trapped, and focus returns to the open control.
  // Hooks are hoisted above the early return (Rules of Hooks) and no-op closed.
  const close = () => playerStore.setQueueOpen(false);
  useOverlay({
    tier: OverlayTier.Menu,
    onClose: close,
    containerRef: panelRef,
    enabled: ui.queueOpen,
  });
  useFocusTrap(panelRef, ui.queueOpen);

  if (!ui.queueOpen) {
    return null;
  }

  const entries = snapshot.queue;
  const hasPending = entries.some((e) => !e.isCurrent);

  function command(action: string) {
    void bridge.call("queue_command", { command: action });
  }

  return (
    <div
      className="queue-panel"
      data-testid="queue-panel"
      role="dialog"
      aria-label="播放队列"
      ref={panelRef}
    >
      <div className="queue-panel-head">
        <span className="queue-panel-title">播放队列</span>
        <button type="button" className="icon-btn" aria-label="关闭队列" onClick={close}>
          ✕
        </button>
      </div>

      {entries.length === 0 ? (
        <div className="queue-empty" data-testid="queue-empty">
          <p className="queue-empty-text">队列为空</p>
          <button type="button" className="btn" onClick={close}>
            浏览曲库
          </button>
        </div>
      ) : (
        <ul className="queue-list" data-testid="queue-list">
          {entries.map((entry) => (
            <li
              key={entry.entryId}
              className={[
                "queue-item",
                entry.isCurrent ? "is-current" : "",
                entry.failed ? "is-failed" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              aria-current={entry.isCurrent ? "true" : undefined}
            >
              <span className="queue-item-glyph" aria-hidden="true">
                {entry.isCurrent ? "▶" : entry.failed ? "⛔" : entry.songId ? "♪" : "📄"}
              </span>
              <span className="queue-item-label">
                {entry.title ?? "歌曲"}
                {entry.canImport ? " " : ""}
                {entry.canImport ? (
                  <span className="queue-item-temporary-tag" aria-label="临时播放项">
                    临时
                  </span>
                ) : null}
              </span>
              <span className="queue-item-id">
                {entry.failed ? "加载失败" : entry.songId ? "资料库歌曲" : "临时文件"}
              </span>
            </li>
          ))}
        </ul>
      )}

      <div className="queue-actions">
        <button
          type="button"
          className="btn"
          disabled={!hasPending}
          onClick={() => command("clearPending")}
        >
          清空待播
        </button>
      </div>
    </div>
  );
}
