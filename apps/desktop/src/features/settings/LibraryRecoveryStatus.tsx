/**
 * 音乐库恢复状态 (tasks 8.2 / 8.3).
 *
 * The settings surface for the durability protocol. It answers one question —
 * "is everything I did actually in my music library, and what is still
 * catching up?" — using the typed outcome the backend now returns
 * (`LibraryRecoveryStatusDto.outcome`), never by inferring from database
 * details:
 *
 *   complete             the directory and the view agree
 *   savedPendingProjection  a commit is saved, the view has not caught up
 *   recoveryPartial      something needs the user (unresolved records) or the
 *                        history cannot be verified at all
 *
 * Read-only, unresolved records and the pending-projection case each get their
 * own copy, and the pending case offers the **same operation** retry
 * (`retry_library_projection` re-projects the existing saved prefix — it never
 * creates a new edit). There is deliberately no legacy-backfill progress bar and
 * no historical repair entry: an unverifiable history is *reported*, not
 * "repaired" into a claim the directory cannot support.
 *
 * The 播放统计 block below is the other half of 8.2: a qualified listen that
 * could not be committed is retried in the background and never interrupts
 * playback, but it must be *visible* — otherwise a play count silently goes
 * missing. It is a read-only list of the pending sessions, path-free, and it
 * never resubmits the original event.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import type {
  LibraryDurabilityOutcomeDto,
  LibraryRecoveryStatusDto,
  PlaybackRetryReasonDto,
  PlaybackRetryStatusDto,
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
  unverifiedConfirmationMetadata: "本地新值附有旧确认记录，等待核对",
  requiresStateRecovery: "原资料已保留，等待专门恢复",
};

const PENDING_REASONS: Record<PlaybackRetryReasonDto, string> = {
  notSaved: "等待保存到音乐库",
  savedPendingRecovery: "已保存，等待恢复",
  savedPendingProjection: "已保存，等待更新视图",
  libraryUnavailable: "音乐库暂不可用",
  inactiveLibrary: "不在当前音乐库",
  authorChanged: "设备身份已改变",
  libraryChanged: "音乐库已更换",
  missingSong: "歌曲已不在音乐库",
  enrollmentPending: "设备同步尚未完成",
};

/** The user-facing sentence for one durability outcome. */
function outcomeMessage(status: LibraryRecoveryStatusDto): string {
  if (status.readOnly) {
    return "音乐库为只读。已保存资料仍可浏览；恢复写入需重新获得目录写入权限。";
  }
  const outcome: LibraryDurabilityOutcomeDto = status.outcome;
  switch (outcome.kind) {
    case "complete":
      return "已保存资料的视图已更新。";
    case "savedPendingProjection":
      return "更改已保存到音乐库，等待更新视图。无需重复导入或编辑。";
    case "savedPendingRecovery":
      return "更改已保存到音乐库，等待恢复。无需再次提交这项操作。";
    case "recoveryPartial":
      return outcome.reason === "legacyUnverified"
        ? "历史资料完整性尚未验证，不能确认喜欢和歌单已全部恢复。"
        : `部分资料待恢复：${status.issues.length} 项原资料已保留。`;
    case "notSaved":
      return "更改未保存到音乐库。请检查目录后重试这项操作。";
  }
}

/** True when the saved prefix is ahead of the projected view. */
function needsProjection(status: LibraryRecoveryStatusDto): boolean {
  return status.outcome.kind === "savedPendingProjection";
}

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
      // The same saved operations, re-projected — never a new edit.
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
  const pending = status !== null && needsProjection(status);
  return (
    <div className="library-recovery-status" aria-label="音乐库恢复状态">
      <div className="setting-row">
        <div className="storage-value">
          <strong>音乐库恢复状态</strong>
          <span className="setting-help" aria-live="polite">
            {status ? outcomeMessage(status) : error ? "恢复状态尚未确认。" : "正在读取恢复状态…"}
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
      {pending ? (
        <button
          type="button"
          className="btn"
          disabled={busy || scanning}
          onClick={() => void recover()}
          aria-describedby="library-recovery-retry-note"
        >
          {busy ? "正在更新已保存资料…" : "更新已保存资料"}
        </button>
      ) : null}
      {pending ? (
        <p className="setting-help" id="library-recovery-retry-note">
          重试的是已经保存的同一项更改，不会创建新的收藏或歌单。
        </p>
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
      <PlaybackRetryDiagnostics root={root} scanning={scanning} />
    </div>
  );
}

/**
 * 播放统计 pending listens — read-only, never a blocking error.
 *
 * A listen is counted only after the library directory commits its event; a
 * failure there must not stop playback (the recorder queues and retries it), but
 * it also must not disappear. This lists what is still waiting, with the reason,
 * so the user can tell "统计还没写进去" apart from "Echo 丢了我的播放".
 *
 * The identity shown is the original playback session; opening this panel never
 * submits a second event for it.
 */
export function PlaybackRetryDiagnostics({
  root,
  scanning,
}: {
  readonly root?: string;
  readonly scanning: boolean;
}) {
  const [pending, setPending] = useState<readonly PlaybackRetryStatusDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);
  const currentRoot = useRef(root);
  currentRoot.current = root;

  const refresh = useCallback(async () => {
    if (!root) {
      setPending(null);
      return;
    }
    try {
      const next = await bridge.call("playback_retry_status", { root });
      if (mounted.current && currentRoot.current === root) {
        setPending(next);
        setError(null);
      }
    } catch {
      if (mounted.current && currentRoot.current === root) {
        setError("暂时无法读取待记录的播放统计。");
      }
    }
  }, [root]);

  useEffect(() => {
    mounted.current = true;
    setPending(null);
    setError(null);
    void refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  if (!root) return null;
  // The count is the only thing that justifies the block: an empty queue is the
  // normal state and needs no reassurance.
  if (pending !== null && pending.length === 0 && error === null) return null;

  return (
    <div className="setting-row" aria-label="待记录的播放统计">
      <div className="storage-value">
        <strong>播放统计</strong>
        <span className="setting-help" aria-live="polite">
          {error
            ? error
            : pending === null
              ? "正在读取播放统计…"
              : `${pending.length} 次播放尚未写入音乐库，Echo 会自动重试，播放不受影响。`}
        </span>
      </div>
      <button
        type="button"
        className="btn"
        disabled={busy || scanning}
        onClick={() => {
          setBusy(true);
          void refresh().finally(() => {
            if (mounted.current) setBusy(false);
          });
        }}
      >
        刷新统计
      </button>
      {pending !== null && pending.length > 0 ? (
        <details>
          <summary>查看待记录的播放</summary>
          <ul>
            {pending.map((entry) => (
              <li key={entry.operationId}>
                <p>{PENDING_REASONS[entry.reason]}</p>
                <small>
                  播放标识：{entry.operationId} · 已重试 {entry.attempts} 次
                </small>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </div>
  );
}
