/** Library workspace types (task 10.4 / 10.5). */

/** The four library views exposed in the shell (task 10.4): the "全部歌曲"
 *  view supports four sorts; "最近添加" and "喜欢的音乐" keep their own
 *  deterministic order; a playlist view is resolved by id. */
export type LibraryViewKind = "all" | "recent" | "favorites" | "playlist";

/** The four sort fields offered in the "全部歌曲" view (task 6.1 / 10.5). */
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
