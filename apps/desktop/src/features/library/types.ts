/** Library workspace types (task 10.4 / 10.5). */

/** The four library views exposed in the shell. */
export type LibraryViewKind = "all" | "recent" | "favorites" | "artists" | "albums" | "playlist";

/** The four sort fields offered in every manually sortable song view. */
export type SongSortField = "addedAt" | "title" | "artist" | "playCount";

export interface SongSort {
  readonly field: SongSortField;
  readonly direction: "asc" | "desc";
}

/** Stable sort order labels shown to the user. */
export const SORT_FIELDS: ReadonlyArray<{ value: SongSortField; label: string }> = [
  { value: "addedAt", label: "最近添加" },
  { value: "title", label: "歌曲名称" },
  { value: "artist", label: "艺人" },
  { value: "playCount", label: "播放次数" },
];
