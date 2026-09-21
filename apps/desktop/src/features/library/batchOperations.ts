import { bridge } from "../../bridge";
import type { SongView } from "../../ipc/ipc-types.generated";

export type BatchItemStatus = "success" | "skipped" | "failed";

export interface BatchItemResult<TItem, TValue = unknown> {
  readonly item: TItem;
  readonly status: BatchItemStatus;
  readonly value?: TValue;
  readonly message?: string;
}

export interface BatchResult<TItem, TValue = unknown> {
  readonly items: readonly BatchItemResult<TItem, TValue>[];
  readonly succeeded: number;
  readonly skipped: number;
  readonly failed: number;
}

export type BatchDecision<TValue> =
  | { readonly status: "success"; readonly value?: TValue }
  | { readonly status: "skipped"; readonly message: string };

export async function runSequentialBatch<TItem, TValue>(
  items: readonly TItem[],
  operation: (item: TItem, index: number) => Promise<BatchDecision<TValue>>,
): Promise<BatchResult<TItem, TValue>> {
  const results: BatchItemResult<TItem, TValue>[] = [];
  for (const [index, item] of items.entries()) {
    try {
      const decision = await operation(item, index);
      results.push(
        decision.status === "success"
          ? { item, status: "success", value: decision.value }
          : { item, status: "skipped", message: decision.message },
      );
    } catch (error) {
      results.push({ item, status: "failed", message: batchErrorMessage(error) });
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
    const committed = await bridge.call("set_favorite", { songId: song.id, favorite });
    return { status: "success", value: committed };
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
  skipped,
  failed,
}: {
  readonly succeeded: number;
  readonly skipped: number;
  readonly failed: number;
}): string {
  const parts = [`成功 ${succeeded}`];
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
  if (code === "unavailable") return "资料库暂不可用";
  if (code === "permission") return "没有操作权限";
  if (code === "conflict") return "当前状态冲突";
  return "操作失败，请重试";
}
