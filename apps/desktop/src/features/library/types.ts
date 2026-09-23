/** Library workspace types (task 10.4 / 10.5). */

/** The four library views exposed in the shell. */
export type LibraryViewKind = "all" | "recent" | "favorites" | "artists" | "albums" | "playlist";

/** The five sort fields offered in the manually sortable all-songs view. */
export type SongSortField = "addedAt" | "title" | "artist" | "album" | "playCount";

export interface SongSort {
  readonly field: SongSortField;
  readonly direction: "asc" | "desc";
}

/** Stable sort order labels shown to the user. */
export const SORT_FIELDS: ReadonlyArray<{ value: SongSortField; label: string }> = [
  { value: "addedAt", label: "最近添加" },
  { value: "title", label: "歌曲名称" },
  { value: "artist", label: "歌手" },
  { value: "album", label: "专辑" },
  { value: "playCount", label: "播放次数" },
];
