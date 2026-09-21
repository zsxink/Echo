/** Public library-feature contract. Cross-feature consumers import only here. */
import { resetLibraryCounts as resetCounts } from "./libraryCounts";
import { resetSongUpdates as resetUpdates } from "./songUpdates";

export { LibraryWorkspace } from "./LibraryWorkspace";
export { SongList } from "./SongList";
export { formatDuration } from "./SongRow";
export { SongMenu } from "./SongMenu";
export { BatchSongMenu, SelectionModeButton } from "./BatchSongActions";
export type { BatchSongActionHandlers, BatchSongActionOptions } from "./BatchSongActions";
export { ConfirmationDialog } from "./ConfirmationDialog";
export type { MenuAnchor, SongMenuProps } from "./SongMenu";
export { SongSortControl } from "./SongSortControl";
export { useStoredSongSort } from "./useStoredSongSort";
export { useSongSelection } from "./useSongSelection";
export type { SongSelection } from "./useSongSelection";
export {
  runAddToPlaylistsBatch,
  runDeleteBatch,
  runFavoriteBatch,
  formatBatchFailureDetails,
  formatBatchResult,
  runQueueBatch,
  runRemoveFromPlaylistBatch,
  runSequentialBatch,
  summarize,
  undoDeleteBatch,
} from "./batchOperations";
export type {
  BatchDecision,
  BatchItemResult,
  BatchItemStatus,
  BatchResult,
  UndoBatchResult,
} from "./batchOperations";
export { coverClass } from "./coverClass";
export {
  bumpLibraryCount,
  invalidateLibraryCounts,
  resetLibraryCounts,
  useLibraryCountSync,
  useLibraryCounts,
} from "./libraryCounts";
export type { LibraryCountView, LibraryCounts } from "./libraryCounts";
export { publishSongUpdate, resetSongUpdates, subscribeSongUpdates } from "./songUpdates";
export type { LibraryViewKind, SongSort } from "./types";

/** Reset feature-owned external state between isolated tests. */
export function resetLibraryState(): void {
  resetCounts();
  resetUpdates();
}
