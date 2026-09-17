/**
 * 播放队列面板 (task 11.2) — the prototype's `.queue-popover`.
 *
 * Renders the authoritative queue from the `PlayerSnapshot` (`snapshot.queue`):
 * the current entry followed by the pending entries, each with a cover block,
 * title + secondary line and duration, and per-entry failed / blocked state
 * surfaced. The header carries the queue size and 清空待播 (clear pending).
 *
 * The snapshot is the single source of truth — this panel never fabricates a
 * queue. Clearing pending removes only *pending* entries: the current song keeps
 * playing and neither the library nor playlists are touched. Blocked
 * (unavailable/missing) and failed entries stay visible so the user understands
 * why an item did not play.
 */

import { useRef, useState } from "react";

import { assetUrl, bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { usePlayerSnapshot, usePlayerUi, playerStore } from "../../player/playerStore";
import { coverClass, formatDuration } from "../library";

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
      src={assetUrl(coverKey)}
      alt=""
      data-testid="queue-cover-image"
      onError={() => setFailed(true)}
    />
  );
}

export function QueuePanel() {
  const snapshot = usePlayerSnapshot();
  const ui = usePlayerUi();
  const panelRef = useRef<HTMLDivElement>(null);
  // Queue panel is a `Menu`-tier overlay: Escape closes it via the single stack,
  // focus is trapped, and focus returns to the open control. Hooks are hoisted
  // above the early return (Rules of Hooks) and no-op while closed.
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
  const hasPending = entries.some((entry) => !entry.isCurrent);

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
          <div className="queue-list" data-testid="queue-list">
            {entries.map((entry) => (
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
                      ? "暂时不可用，可在资料库恢复后重试"
                      : entry.failed
                        ? "加载失败"
                        : (entry.artist ?? (entry.canImport ? "临时文件" : "未知艺人"))}
                    {entry.canImport ? (
                      <span className="queue-temporary-tag" aria-label="临时播放项">
                        临时
                      </span>
                    ) : null}
                  </span>
                </div>
                <span className="queue-time">
                  {entry.durationS === null ? "时长未知" : formatDuration(entry.durationS)}
                </span>
              </button>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
