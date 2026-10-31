/**
 * 已保存待更新的统一反馈 (task 8.2).
 *
 * A durability failure has two very different meanings, and the UI must never
 * conflate them:
 *
 *   未保存   the library directory rejected the change. The optimistic state is
 *            a lie: roll it back and show a retryable reason.
 *   已保存   the directory committed it; only the SQLite projection lags. The
 *            user's intent *happened*. Rolling it back would invite a duplicate
 *            edit, so the committed truth stays on screen and the retry is by
 *            the original operation identity — never a second mutation.
 *
 * `savedLibraryOperation` is the single classifier (structured
 * `durabilityOutcome` first, then the error code). This module turns that
 * verdict into the feedback every surface shares, so no call site re-derives it
 * from a message string.
 *
 * The toast is never the only durable-failure surface (design D7): settings'
 * 音乐库恢复状态 keeps the persistent record. The toast only carries the
 * immediate, actionable retry.
 */

import { bridge } from "../../bridge";
import { savedLibraryOperation } from "../../bridge/libraryPersistence";
import { notify } from "../../app/toast";

import { formatBatchFailureDetails, formatBatchResult } from "./batchOperations";
import type { BatchResult } from "./batchOperations";
import { invalidateLibraryCounts } from "./libraryCounts";
import { invalidateLibrary } from "./libraryInvalidation";

/** Options shared by the saved-pending feedback helpers. */
export interface SavedPendingOptions {
  /** Active root; without it the retry cannot be offered. */
  readonly root?: string;
  /** Runs after a successful recovery (re-query, re-count, close a dialog). */
  readonly onRecovered?: () => void;
}

/**
 * Report a mutation failure, distinguishing saved-but-waiting from not-saved.
 *
 * Returns `true` when the change was already committed to the library, which is
 * the caller's signal to keep the optimistic state instead of rolling it back.
 */
export function notifySavedPending(error: unknown, options: SavedPendingOptions = {}): boolean {
  const saved = savedLibraryOperation(error);
  if (!saved) return false;
  const { operationId } = saved;
  const { root, onRecovered } = options;
  if (operationId && root) {
    // The identity is the one the directory already recorded: retrying it
    // re-projects that same change, it does not submit a new one.
    notify({
      message: saved.message,
      actionLabel: "恢复",
      onAction: () => {
        void retrySavedOperations(root, [operationId], onRecovered);
      },
    });
  } else {
    notify(saved.message);
  }
  return true;
}

/**
 * Recover already-committed operations by their original identity.
 *
 * Deliberately sequential: each retry is a directory read plus a projection
 * write, and the settings/import paths use the same one-at-a-time contract.
 * Failures stay queued — the change is still saved — so this reports what it
 * could not catch up rather than pretending the edits were lost.
 */
export async function retrySavedOperations(
  root: string,
  operations: readonly string[],
  onRecovered?: () => void,
): Promise<void> {
  const unrecovered: string[] = [];
  let recovered = 0;
  for (const operation of operations) {
    try {
      await bridge.call("retry_library_operation", { root, operation });
      recovered += 1;
    } catch {
      unrecovered.push(operation);
    }
  }
  if (recovered > 0) {
    invalidateLibrary();
    invalidateLibraryCounts();
    onRecovered?.();
  }
  if (unrecovered.length > 0) {
    notify({
      message:
        recovered > 0
          ? `已恢复 ${recovered} 项，另有 ${unrecovered.length} 项已保存在音乐库中，可在设置中重试。`
          : "更改已保存在音乐库中，但更新尚未完成。可在设置中查看并重试。",
    });
    return;
  }
  if (recovered > 0) notify(`已从音乐库恢复 ${recovered} 项`);
}

/**
 * The batch toast: per-item counts, plus a same-operation retry when some items
 * are committed but still waiting for the projection.
 *
 * `failed` alone decides the error styling — a pending item is saved, so
 * painting the toast red would tell the user a committed change was lost.
 */
export function notifyBatchOutcome<TItem, TValue>(
  result: BatchResult<TItem, TValue>,
  options: {
    readonly root?: string;
    readonly label?: (item: TItem) => string;
    readonly onRecovered?: () => void;
  } = {},
): void {
  const { root, label, onRecovered } = options;
  const details = formatBatchFailureDetails(result, label);
  const operations = result.items.flatMap((item) =>
    item.status === "pending" && item.operationId ? [item.operationId] : [],
  );
  const base = `批量操作：${formatBatchResult(result)}${details ? `。${details}` : ""}`;
  if (operations.length === 0 || !root) {
    notify({ message: base, error: result.failed > 0 });
    return;
  }
  // Sequential by operation identity — each retry re-projects that same
  // committed change; none of them submits a new one.
  notify({
    message: `${base}。已保存的 ${operations.length} 项等待更新视图。`,
    error: result.failed > 0,
    actionLabel: "恢复",
    onAction: () => void retrySavedOperations(root, operations, onRecovered),
  });
}
