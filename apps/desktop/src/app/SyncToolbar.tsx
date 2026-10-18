import { useEffect, useId, useRef, useState } from "react";

import { lanSyncAdapter } from "../features/sync/lanSyncAdapter";
import type { SyncStatus } from "../ipc/ipc-types.generated";
import { Icon } from "./Icon";
import { OverlayTier, useOverlay } from "./overlays";
import "./syncToolbar.css";

/** Completion is reported by native resource publication, never inferred from connectivity. */
function statusLabel(status: SyncStatus | null): string {
  if (!status) return "正在读取同步状态";
  if (!status.enabled) return "同步已关闭";
  if (status.state === "paused") return "同步已暂停";
  if (status.lastError || status.state === "failed") {
    return status.lastError === "sync.connectionFailed" ? "主设备离线" : "同步需要重试";
  }
  if ((status.pendingUploads ?? 0) > 0) return "正在上传";
  if ((status.pendingDownloads ?? 0) > 0) return "正在下载";
  if (status.state === "syncing") return "正在应用变更";
  if ((status.pendingChanges ?? 0) > 0) return "仍有待同步";
  if (status.mediaReady !== true) return "等待媒体就绪";
  if (status.pendingChanges !== 0 || status.pendingUploads !== 0 || status.pendingDownloads !== 0) {
    return "正在核对同步状态";
  }
  return "已同步";
}

function errorLabel(key: string): string {
  const messages: Record<string, string> = {
    "sync.connectionFailed": "无法连接主设备，请确认主设备在线且处于同一局域网。",
    "sync.resourcePublicationPending": "媒体尚未就绪，请检查磁盘空间和媒体目录权限后重试。",
    "sync.libraryUnavailable": "资料库不可用，请确认音乐目录仍可访问。",
    "sync.authorizationFailed": "设备授权已失效，请在设置中重新加入主设备。",
    "sync.protocolFailed": "同步数据不兼容，请确认两端 Echo 版本一致。",
    "sync.hostUnavailable": "主设备暂时不可用，请检查主设备的同步服务。",
  };
  return messages[key] ?? "同步未完成，请重试或在设置中检查同步配置。";
}

function bytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function TransferProgress(props: {
  label: string;
  pending: number | null | undefined;
  completed: number | null | undefined;
  total: number | null | undefined;
}) {
  const total = props.total ?? 0;
  const completed = props.completed == null ? undefined : Math.min(props.completed, total);
  return (
    <div className="sync-transfer">
      <span>
        {props.label}：{props.pending == null ? "待统计" : `${props.pending} 项待处理`}
      </span>
      {total > 0 ? (
        <>
          <progress aria-label={`${props.label}进度`} value={completed} max={total} />
          <span className="sync-transfer-bytes">
            {completed == null ? "进度待统计" : bytes(completed)} / {bytes(total)}
          </span>
        </>
      ) : null}
    </div>
  );
}

export function SyncToolbar() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const latest = useRef<SyncStatus | null>(null);
  const mounted = useRef(false);
  const actionSequence = useRef(0);
  const panel = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const headingId = useId();

  function accept(next: SyncStatus) {
    const previous = latest.current;
    if (
      previous &&
      (next.sessionId < previous.sessionId ||
        (next.sessionId === previous.sessionId && next.eventSequence <= previous.eventSequence))
    ) {
      return;
    }
    latest.current = next;
    setStatus(next);
    setError(null);
  }

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void lanSyncAdapter
      .subscribeStatus((next) => {
        if (!disposed) accept(next);
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        if (!disposed) setError("无法订阅同步状态，请重试。");
      });
    void lanSyncAdapter
      .status()
      .then((next) => {
        if (!disposed) accept(next);
      })
      .catch(() => {
        if (!disposed) setError("无法读取同步状态，请重试。");
      });
    return () => {
      disposed = true;
      mounted.current = false;
      unlisten?.();
    };
  }, []);

  useOverlay({
    tier: OverlayTier.Menu,
    containerRef: panel,
    triggerRef: trigger,
    enabled: open,
    onClose: () => setOpen(false),
    nonModal: true,
  });

  async function run(action: "sync" | "pause" | "resume" | "read") {
    const sequence = ++actionSequence.current;
    setPendingAction(action);
    setError(null);
    try {
      const next = await {
        sync: lanSyncAdapter.syncNow,
        pause: lanSyncAdapter.pause,
        resume: lanSyncAdapter.resume,
        read: lanSyncAdapter.status,
      }[action]();
      if (mounted.current) accept(next);
    } catch {
      if (mounted.current && sequence === actionSequence.current) {
        setError("同步操作未完成，请重试。");
      }
    } finally {
      if (mounted.current && sequence === actionSequence.current) setPendingAction(null);
    }
  }

  const label = error && !status ? "无法读取同步状态" : statusLabel(status);
  const paused = status?.state === "paused";
  const retry = Boolean(error || status?.lastError || status?.state === "failed");
  return (
    <div className="sync-toolbar">
      <button
        ref={trigger}
        type="button"
        className="sync-status-button"
        data-testid="sync-button"
        aria-expanded={open}
        aria-controls={panelId}
        aria-label={`资料库同步：${label}`}
        title={`资料库同步：${label}`}
        onClick={() => setOpen((value) => !value)}
      >
        <Icon name={label === "已同步" ? "check" : "recent"} />
        <span className="sync-status-label" aria-live="polite">
          {label}
        </span>
      </button>
      {open ? (
        <div
          ref={panel}
          className="sync-status-panel"
          id={panelId}
          role="dialog"
          aria-labelledby={headingId}
          tabIndex={-1}
        >
          <strong id={headingId}>资料库同步</strong>
          <p>{label}</p>
          {status?.enabled ? (
            <>
              <p>
                待确认修改：
                {status.pendingChanges == null ? "待统计" : `${status.pendingChanges} 项`}
              </p>
              <TransferProgress
                label="上传"
                pending={status.pendingUploads}
                completed={status.uploadedBytes}
                total={status.totalUploadBytes}
              />
              <TransferProgress
                label="下载"
                pending={status.pendingDownloads}
                completed={status.downloadedBytes}
                total={status.totalDownloadBytes}
              />
              {status.mediaReady !== true ? <p>媒体全部就绪后才会显示“已同步”。</p> : null}
            </>
          ) : (
            <p>在设置中选择“作为主设备”或“加入主设备”以启用同步。</p>
          )}
          {error || status?.lastError ? (
            <p role="alert">{error ?? errorLabel(status!.lastError!)}</p>
          ) : null}
          <div className="sync-status-actions">
            {status?.enabled ? (
              <>
                <button
                  type="button"
                  className="btn"
                  disabled={pendingAction !== null || (status.state === "syncing" && !retry)}
                  onClick={() => void run(paused ? "resume" : "sync")}
                >
                  {paused ? "恢复同步" : retry ? "重试同步" : "立即同步"}
                </button>
                {!paused ? (
                  <button
                    type="button"
                    className="btn"
                    disabled={pendingAction === "pause" || pendingAction === "resume"}
                    onClick={() => void run("pause")}
                  >
                    暂停同步
                  </button>
                ) : null}
              </>
            ) : retry ? (
              <button
                type="button"
                className="btn"
                disabled={pendingAction !== null}
                onClick={() => void run("read")}
              >
                重试读取
              </button>
            ) : null}
            <button type="button" className="btn" onClick={() => setOpen(false)}>
              关闭
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
