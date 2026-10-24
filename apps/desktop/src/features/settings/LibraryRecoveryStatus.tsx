import { useCallback, useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import type {
  LibraryRecoveryStatusDto,
  RecoveryIssueReasonDto,
} from "../../ipc/ipc-types.generated";
import { invalidateLibrary } from "../library";

const REASONS: Record<RecoveryIssueReasonDto, string> = {
  missingSong: "缺少关联的歌曲资料",
  missingPlaylist: "缺少关联的歌单资料",
  identityConflict: "存在需要核对的身份冲突",
  tombstonedDependency: "关联的资料已有删除记录",
  missingSyncDependency: "缺少同步所需的关联资料",
  ambiguousMutationVersion: "存在无法确认版本的同步更改",
  invalidStateFact: "存在无效的同步状态资料",
  requiresStateRecovery: "原资料已保留，等待专门恢复",
};

/** Read retained evidence each time settings reopens; never retry the original edit. */
export function LibraryRecoveryStatus({
  root,
  scanning,
}: {
  readonly root?: string;
  readonly scanning: boolean;
}) {
  const [status, setStatus] = useState<LibraryRecoveryStatusDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const revision = useRef(0);
  const mounted = useRef(true);
  const currentRoot = useRef(root);
  currentRoot.current = root;
  const invalidate = useCallback(() => ++revision.current, []);

  const refresh = useCallback(async () => {
    const request = invalidate();
    if (!root) {
      setStatus(null);
      return;
    }
    try {
      const next = await bridge.call("library_recovery_status", { root });
      if (
        mounted.current &&
        currentRoot.current === root &&
        request === revision.current &&
        next.root === root
      ) {
        setStatus(next);
        setError(null);
      }
    } catch {
      if (mounted.current && currentRoot.current === root && request === revision.current) {
        setError("暂时无法读取音乐库恢复状态。资料未被删除，请检查目录后刷新。");
      }
    }
  }, [root, invalidate]);

  useEffect(() => {
    mounted.current = true;
    setStatus(null);
    setError(null);
    setBusy(false);
    void refresh();
    return () => {
      mounted.current = false;
      invalidate();
    };
  }, [refresh, invalidate]);

  async function recover() {
    if (!root || busy || scanning) return;
    const requestRoot = root;
    setBusy(true);
    setError(null);
    try {
      await bridge.call("retry_library_projection", { root: requestRoot });
      if (mounted.current && currentRoot.current === requestRoot) {
        invalidateLibrary();
        await refresh();
      }
    } catch {
      if (mounted.current && currentRoot.current === requestRoot)
        setError("已保存资料仍在音乐库中，更新尚未完成。请检查目录后重试。");
    } finally {
      if (mounted.current && currentRoot.current === requestRoot) setBusy(false);
    }
  }

  if (!root) return null;
  return (
    <div className="library-recovery-status" aria-label="音乐库恢复状态">
      <div className="setting-row">
        <div className="storage-value">
          <strong>音乐库恢复状态</strong>
          <span className="setting-help" aria-live="polite">
            {status?.needsProjection
              ? "更改已保存到音乐库，等待更新视图。无需重复导入或编辑。"
              : status?.issues.length
                ? `部分资料待恢复：${status.issues.length} 项原资料已保留。`
                : status?.legacyUnverified
                  ? "历史资料完整性尚未验证，不能确认喜欢和歌单已全部恢复。"
                  : status
                    ? "已保存资料的视图已更新。"
                    : error
                      ? "恢复状态尚未确认。"
                      : "正在读取恢复状态…"}
          </span>
        </div>
        <button
          type="button"
          className="btn"
          disabled={busy || scanning}
          onClick={() => void refresh()}
        >
          刷新状态
        </button>
      </div>
      {status && (status.needsProjection || status.issues.length > 0) ? (
        <button
          type="button"
          className="btn"
          disabled={busy || scanning}
          onClick={() => void recover()}
        >
          {busy ? "正在更新已保存资料…" : "更新已保存资料"}
        </button>
      ) : null}
      {error ? (
        <p className="setting-help" role="alert">
          {error}
        </p>
      ) : null}
      {status && status.issues.length > 0 ? (
        <details>
          <summary>查看待恢复资料</summary>
          <ul>
            {status.issues.map((issue) => (
              <li key={issue.target}>
                <p>{REASONS[issue.reason]}</p>
                <small>资料标识：{issue.target}</small>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </div>
  );
}
