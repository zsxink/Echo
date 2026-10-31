import { bridge } from "../../bridge";
import { savedLibraryOperation } from "../../bridge/libraryPersistence";
import type { SongView } from "../../ipc/ipc-types.generated";

/**
 * `pending` is the fifth outcome the durability protocol needs: the library
 * directory committed the item, but the projection has not caught up yet. It is
 * deliberately **not** `failed` — the change is saved, the user must not retry
 * it as a new intent, and the batch summary must not report it as a loss.
 */
export type BatchItemStatus = "success" | "pending" | "skipped" | "failed";

export interface BatchItemResult<TItem, TValue = unknown> {
  readonly item: TItem;
  readonly status: BatchItemStatus;
  readonly value?: TValue;
  readonly message?: string;
  /** The stable operation identity of a committed-but-pending item. */
  readonly operationId?: string;
}

export interface BatchResult<TItem, TValue = unknown> {
  readonly items: readonly BatchItemResult<TItem, TValue>[];
  readonly succeeded: number;
  /** Items the library committed but whose view has not caught up yet. */
  readonly pending: number;
  readonly skipped: number;
  readonly failed: number;
}

export type BatchDecision<TValue> =
  | { readonly status: "success"; readonly value?: TValue }
  | { readonly status: "skipped"; readonly message: string }
  /**
   * The library committed this item but its projection has not caught up.
   * `value` carries the committed truth the caller may publish locally (never a
   * guess — the directory is the authority once the commit succeeds).
   */
  | {
      readonly status: "pending";
      readonly value?: TValue;
      readonly message: string;
      readonly operationId?: string;
    };

export async function runSequentialBatch<TItem, TValue>(
  items: readonly TItem[],
  operation: (item: TItem, index: number) => Promise<BatchDecision<TValue>>,
): Promise<BatchResult<TItem, TValue>> {
  const results: BatchItemResult<TItem, TValue>[] = [];
  for (const [index, item] of items.entries()) {
    try {
      const decision = await operation(item, index);
      if (decision.status === "pending") {
        results.push({
          item,
          status: "pending",
          value: decision.value,
          message: decision.message,
          operationId: decision.operationId,
        });
      } else {
        results.push(
          decision.status === "success"
            ? { item, status: "success", value: decision.value }
            : { item, status: "skipped", message: decision.message },
        );
      }
    } catch (error) {
      // A directory-committed mutation that only failed to project is saved:
      // it keeps its own status and identity, and the batch never rolls it back
      // as if the library had rejected it (spec: 单项已保存待投影不得计为失败).
      const saved = savedLibraryOperation(error);
      results.push(
        saved
          ? {
              item,
              status: "pending",
              message: saved.message,
              operationId: saved.operationId,
            }
          : { item, status: "failed", message: batchErrorMessage(error) },
      );
    }
  }
  return summarize(results);
}

export function summarize<TItem, TValue>(
  items: readonly BatchItemResult<TItem, TValue>[],
): BatchResult<TItem, TValue> {
  return {
    items,
    succeeded: items.filter((item) => item.status === "success").length,
    pending: items.filter((item) => item.status === "pending").length,
    skipped: items.filter((item) => item.status === "skipped").length,
    failed: items.filter((item) => item.status === "failed").length,
  };
}

export async function runFavoriteBatch(
  songs: readonly SongView[],
  favorite: boolean,
): Promise<BatchResult<SongView, SongView>> {
  return runSequentialBatch(songs, async (song) => {
    if (song.availability !== "available") {
      return { status: "skipped", message: "歌曲不可用" };
    }
    if (song.favorite === favorite) {
      return { status: "skipped", message: favorite ? "已经收藏" : "尚未收藏" };
    }
    try {
      const committed = await bridge.call("set_favorite", { songId: song.id, favorite });
      return { status: "success", value: committed };
    } catch (error) {
      const saved = savedLibraryOperation(error);
      if (!saved) throw error;
      // The directory committed the like; only the view lags. The committed
      // truth is the requested `favorite`, so the item keeps that state and the
      // caller can publish it locally instead of rolling the heart back.
      return {
        status: "pending",
        value: { ...song, favorite },
        message: saved.message,
        operationId: saved.operationId,
      };
    }
  });
}

export async function runAddToPlaylistsBatch(
  songs: readonly SongView[],
  targets: readonly string[],
): Promise<BatchResult<SongView>> {
  return runSequentialBatch(songs, async (song) => {
    if (song.availability !== "available") {
      return { status: "skipped", message: "歌曲不可用" };
    }
    try {
      await bridge.call("add_to_playlists", { song: song.id, targets: [...targets] });
      return { status: "success" };
    } catch (error) {
      if (codeOf(error) === "conflict") return { status: "skipped", message: "已在歌单中" };
      throw error;
    }
  });
}

export async function runRemoveFromPlaylistBatch(
  playlistId: string,
  songs: readonly SongView[],
): Promise<BatchResult<SongView>> {
  return runSequentialBatch(songs, async (song) => {
    await bridge.call("remove_playlist_song", { playlist: playlistId, song: song.id });
    return { status: "success" };
  });
}

export async function runQueueBatch(
  songs: readonly SongView[],
  command: "enqueue" | "playNext",
): Promise<BatchResult<SongView>> {
  const ordered = command === "playNext" ? [...songs].reverse() : [...songs];
  const result = await runSequentialBatch(ordered, async (song) => {
    if (song.availability !== "available") {
      return { status: "skipped", message: "歌曲不可用" };
    }
    await bridge.call("queue_command", { command, songId: song.id });
    return { status: "success" };
  });
  const byId = new Map(result.items.map((item) => [item.item.id, item]));
  return summarize(songs.map((song) => byId.get(song.id) ?? { item: song, status: "failed" }));
}

export async function runDeleteBatch(
  root: string,
  songs: readonly SongView[],
): Promise<BatchResult<SongView, string>> {
  return runSequentialBatch(songs, async (song) => {
    if (song.availability !== "available") {
      return { status: "skipped", message: `${availabilityLabel(song)}，无法删除` };
    }
    const operation = await bridge.call("delete_song", { root, song: song.id });
    return { status: "success", value: operation };
  });
}

export interface UndoBatchResult {
  readonly succeeded: number;
  readonly failed: number;
}

export async function undoDeleteBatch(
  root: string,
  operations: readonly string[],
): Promise<UndoBatchResult> {
  let succeeded = 0;
  let failed = 0;
  for (const operation of operations) {
    try {
      await bridge.call("undo_delete", { root, operation });
      succeeded += 1;
    } catch {
      failed += 1;
    }
  }
  return { succeeded, failed };
}

export function formatBatchResult({
  succeeded,
  pending,
  skipped,
  failed,
}: {
  readonly succeeded: number;
  readonly pending: number;
  readonly skipped: number;
  readonly failed: number;
}): string {
  const parts = [`成功 ${succeeded}`];
  // "已保存，等待更新" is neither a success nor a loss — naming it separately is
  // what keeps a committed item from being counted as a failure.
  if (pending > 0) parts.push(`已保存待更新 ${pending}`);
  if (skipped > 0) parts.push(`跳过 ${skipped}`);
  if (failed > 0) parts.push(`失败 ${failed}`);
  return parts.join("，");
}

/** Add actionable failure reasons to a result without changing the compact
 * success copy used by the toast. Batch commands are deliberately best-effort
 * per song, so a plain “失败 N” otherwise makes a real IPC error look like a
 * menu no-op. */
export function formatBatchFailureDetails<TItem>(
  result: BatchResult<TItem>,
  label: (item: TItem) => string = () => "歌曲",
): string {
  return result.items
    .filter((item) => item.status === "failed")
    .map((item) => `${label(item.item)}：${item.message ?? "操作失败，请重试"}`)
    .join("；");
}

/** The committed-but-pending items of a batch, for a caller that wants to
 *  offer the same-operation retry or publish the saved truth locally. */
export function pendingBatchItems<TItem, TValue>(
  result: BatchResult<TItem, TValue>,
): readonly BatchItemResult<TItem, TValue>[] {
  return result.items.filter((item) => item.status === "pending");
}

function availabilityLabel(song: SongView): string {
  return song.availability === "missing" ? "歌曲不可用" : "歌曲已待删除";
}

function codeOf(error: unknown): string | null {
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code?: unknown }).code;
    return typeof code === "string" ? code : null;
  }
  return null;
}

function batchErrorMessage(error: unknown): string {
  const code = codeOf(error);
  if (code === "unavailable") return "音乐库暂不可用";
  if (code === "permission") return "没有操作权限";
  if (code === "conflict") return "当前状态冲突";
  return "操作失败，请重试";
}
