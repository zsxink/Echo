import { useEffect, useState } from "react";

import type { SyncStatus, SyncStorageSpace } from "../../ipc/ipc-types.generated";
import { lanSyncAdapter } from "./lanSyncAdapter";

function bytes(value: number): string {
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GB`;
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${value} B`;
}

function knownBytes(value: number | null | undefined): value is number {
  return value != null && Number.isSafeInteger(value) && value >= 0;
}

/** Capacity is advisory: transfers and backup staging may need additional space. */
export function SyncStorageHint({
  libraryRoot,
  status,
}: {
  readonly libraryRoot?: string;
  readonly status: SyncStatus | null;
}) {
  const [capacity, setCapacity] = useState<SyncStorageSpace | null>(null);
  const [loading, setLoading] = useState(false);
  const sessionId = status?.sessionId;
  const enabled = status?.enabled;

  useEffect(() => {
    if (!libraryRoot) return;
    let disposed = false;
    let inFlight = false;
    const refresh = async () => {
      if (inFlight) return;
      inFlight = true;
      setLoading(true);
      try {
        const next = await lanSyncAdapter.storageSpace(libraryRoot);
        if (!disposed) setCapacity(next?.libraryRoot === libraryRoot ? next : null);
      } catch {
        if (!disposed) setCapacity(null);
      } finally {
        inFlight = false;
        if (!disposed) setLoading(false);
      }
    };
    void refresh();
    const timer = enabled ? window.setInterval(() => void refresh(), 10_000) : undefined;
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [libraryRoot, sessionId, enabled]);

  const available = capacity?.libraryRoot === libraryRoot ? capacity?.availableBytes : null;
  const total = status?.totalDownloadBytes;
  const downloaded = status?.downloadedBytes;
  const remaining =
    knownBytes(total) && knownBytes(downloaded) && downloaded <= total ? total - downloaded : null;
  const insufficient = knownBytes(available) && remaining !== null && available < remaining;

  return (
    <div
      className="settings-note"
      data-testid="sync-storage-hint"
      role={insufficient ? "alert" : undefined}
    >
      {!libraryRoot
        ? "加入主机前请先选择音乐目录，以便检查可用空间。"
        : knownBytes(available)
          ? `当前音乐目录所在磁盘可用 ${bytes(available)}。`
          : loading
            ? "正在检查音乐目录所在磁盘的可用空间…"
            : "无法读取音乐目录所在磁盘的可用空间，请在系统中确认后再加入主机。"}
      {remaining === null
        ? " 主机媒体大小将在连接后统计；加入前请为完整资料库预留空间。"
        : ` 本轮尚需接收 ${bytes(remaining)} 媒体。`}
      {insufficient ? " 当前可用空间不足，请释放空间后重试同步。" : ""}
      {" 临时文件、备份和资料库索引还需要额外空间；此提示不保证同步一定成功。"}
    </div>
  );
}
