import { useCallback, useEffect, useState } from "react";

import type { SyncConflictResolution, SyncConflictView } from "../../ipc/ipc-types.generated";
import { lanSyncAdapter } from "./lanSyncAdapter";

function recoveryLabel(recovery: SyncConflictView["recovery"]): string {
  switch (recovery) {
    case "available":
      return "可恢复普通字段";
    case "pending_edit":
      return "本机仍有待确认修改，暂不能恢复";
    case "final_deleted":
      return "对象已最终删除；重新导入会创建新 UUID";
    case "unsupported":
      return "该冲突类型只支持保留当前值";
    case "unavailable":
      return "对象或冲突已不可用";
  }
}

function resolutionLabel(result: SyncConflictResolution): string {
  switch (result) {
    case "recovered":
      return "已作为一条新的本机修改排队。";
    case "dismissed":
      return "已保留主机当前值。";
    case "already_resolved":
      return "此冲突已处理。";
    case "final_deleted":
      return "对象已最终删除；如需恢复，请重新导入并创建新 UUID。";
    case "pending_edit":
      return "本机还有待确认修改，当前没有覆盖它。";
    case "unsupported":
      return "此类冲突不能安全恢复，只能保留当前值。";
    case "unavailable":
      return "冲突已不可用，资料库内容未改变。";
  }
}

function pretty(value: unknown): string {
  return JSON.stringify(value, null, 2) ?? "{}";
}

/** Host-local conflict history. Historical values are display-only until the
 * user explicitly restores an allowlisted ordinary field as a fresh mutation. */
export function SyncConflictsPanel({ libraryRoot }: { readonly libraryRoot: string }) {
  const [conflicts, setConflicts] = useState<readonly SyncConflictView[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyConflict, setBusyConflict] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setConflicts(await lanSyncAdapter.syncConflicts(libraryRoot, 100));
    } catch {
      setError("无法读取当前资料库的同步冲突。");
    } finally {
      setLoading(false);
    }
  }, [libraryRoot]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const resolve = async (conflict: SyncConflictView, recoverReplaced: boolean) => {
    if (busyConflict) return;
    setBusyConflict(conflict.conflictId);
    setError(null);
    setNotice(null);
    try {
      const result = await lanSyncAdapter.resolveSyncConflict(
        libraryRoot,
        conflict.conflictId,
        recoverReplaced,
      );
      setNotice(resolutionLabel(result));
      await refresh();
    } catch {
      setError("处理冲突失败；资料库未被强制覆盖，请刷新后重试。");
    } finally {
      setBusyConflict(null);
    }
  };

  return (
    <section className="sync-conflicts" aria-labelledby="sync-conflicts-title">
      <div className="sync-conflicts-heading">
        <div>
          <h3 id="sync-conflicts-title">同步冲突</h3>
          <p className="setting-help">显示主机保存的冲突历史；恢复会创建新的本机修改。</p>
        </div>
        <button
          type="button"
          className="btn"
          onClick={() => void refresh()}
          disabled={loading || busyConflict !== null}
          data-testid="sync-conflicts-refresh"
        >
          刷新
        </button>
      </div>
      {error ? <p role="alert">{error}</p> : null}
      {notice ? <p role="status">{notice}</p> : null}
      {loading ? <p className="setting-help">正在读取冲突…</p> : null}
      {!loading && !error && conflicts.length === 0 ? (
        <p className="setting-help">当前资料库没有待处理的同步冲突。</p>
      ) : null}
      <ul className="sync-conflicts-list">
        {conflicts.map((conflict) => {
          const busy = busyConflict === conflict.conflictId;
          return (
            <li key={conflict.conflictId} data-testid="sync-conflict">
              <h4>
                {conflict.objectType} · {conflict.objectUuid}
              </h4>
              <p className="setting-help">
                主机确认次序：{conflict.replacedVersion} → {conflict.acceptedVersion}；
                {recoveryLabel(conflict.recovery)}
              </p>
              <div className="sync-conflict-values">
                <div>
                  <strong>被替代的普通字段</strong>
                  <pre>{pretty(conflict.replacedFields)}</pre>
                </div>
                <div>
                  <strong>主机当前字段</strong>
                  <pre>{pretty(conflict.acceptedFields)}</pre>
                </div>
              </div>
              <div className="setting-choices">
                {conflict.recovery === "available" ? (
                  <button
                    type="button"
                    className="btn"
                    disabled={busyConflict !== null}
                    onClick={() => void resolve(conflict, true)}
                    data-testid="sync-conflict-recover"
                  >
                    {busy ? "正在恢复…" : "恢复被替代字段"}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="btn"
                  disabled={busyConflict !== null}
                  onClick={() => void resolve(conflict, false)}
                  data-testid="sync-conflict-dismiss"
                >
                  保留主机当前值
                </button>
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
