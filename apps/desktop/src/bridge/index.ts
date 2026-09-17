/**
 * Unique Tauri IPC bridge (task 10.2).
 *
 * This is the single module that calls `invoke`/`listen` on the Tauri runtime.
 * No component invokes an arbitrary string command or reaches past this
 * boundary — every command has a typed, generated signature (the camelCase
 * DTOs in `../ipc/ipc-types.generated.ts`) and every result is unwrapped from
 * the `IpcResult<T>` envelope into `T | IpcErrorDto`.
 *
 * The bridge also carries the **Core query cache / player external store**
 * responsibilities off the components: commands go through here, events are
 * de-duplicated by sequence watermark, and stale responses never overwrite a
 * newer revision. Components render from typed state, never from raw IPC.
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { IpcErrorDto } from "../ipc/ipc-types.generated";

/** A typed command name → signature. Keep in sync with the Rust `AppServices`
 *  surface; the generator produces the DTO shapes. */
export interface BridgeCommandMap {
  get_bootstrap_state: () => unknown;
  library_status: () => unknown;
  all_songs: (args: { sort: string; cursor?: string | null; limit: number }) => unknown;
  search: (args: {
    query: string;
    in_favorites: boolean;
    sort: string;
    cursor?: string | null;
    limit: number;
  }) => unknown;
  favorites: (args: { sort: string; cursor?: string | null; limit: number }) => unknown;
  recent: (args: { query: string }) => unknown;
  /** Per-view song totals, answered by Core without opening any view — the
   *  sidebar needs them before the user clicks. */
  library_counts: () => unknown;
  playlists: () => unknown;
  playlist_members: (args: { playlistId: string }) => unknown;
  song_detail: (args: { songId: string }) => unknown;
  /** The opaque cover-asset keys of a batch of songs (design §115 内置优先).
   *  A song with no embedded artwork is absent from the returned map. */
  song_cover_keys: (args: { songIds: string[] }) => unknown;
  set_favorite: (args: { songId: string; favorite: boolean }) => unknown;
  create_playlist: (args: { root: string; name: string }) => unknown;
  rename_playlist: (args: { id: string; name: string }) => unknown;
  /** `bytes: null` clears a manually selected cover and restores auto artwork. */
  set_playlist_cover: (args: {
    id: string;
    bytes: number[] | null;
    mime?: string | null;
  }) => unknown;
  delete_playlist: (args: { id: string }) => unknown;
  add_to_playlists: (args: { song: string; targets: string[] }) => unknown;
  remove_playlist_song: (args: { playlist: string; song: string }) => unknown;
  delete_song: (args: { root: string; song: string }) => unknown;
  undo_delete: (args: { root: string; operation: string }) => unknown;
  choose_library_root: () => unknown;
  choose_and_import_files: () => unknown;
  reveal_song: (args: { songId: string }) => unknown;
  start_scan: (args: { root: string }) => unknown;
  cancel_scan: (args: { root: string }) => unknown;
  set_theme: (args: { theme: string }) => unknown;
  set_close_behavior: (args: { behavior: string }) => unknown;
  get_close_behavior: () => string;
  // Player commands (task 11.1) — the UI sends coarse requests; the Rust
  // coordinator owns the queue + snapshot authority. Playback contexts are
  // resolved on the desktop: the UI submits only a view/selected song, never
  // a song-id list (spec: 视图播放重建队列数量 — a paged/partial client list
  // must not truncate the queue).
  play_playlist_context: (args: { playlist: string; selectedSong: string }) => unknown;
  play_library_context: (args: {
    view: "all" | "recent" | "favorites";
    query: string;
    sort: string;
    selectedSong: string;
  }) => unknown;
  restore_playback_session: () => unknown;
  play_temporary_file: (args: { path: string; displayName: string }) => unknown;
  import_current_temporary_file: () => unknown;
  player_control: (args: { action: string }) => unknown;
  queue_command: (args: { command: string; songId?: string; entryId?: string }) => unknown;
  set_volume: (args: { volume: number }) => unknown;
  toggle_mute: () => unknown;
  seek: (args: { position: number }) => unknown;
  get_lyrics: (args: { songId: string }) => unknown;
}

/** An unwrapper that treats the error envelope as a thrown value. */
export class BridgeError extends Error {
  readonly code: string;
  readonly retryable: boolean;
  readonly operationId?: string;
  readonly field?: string;

  constructor(dto: IpcErrorDto) {
    super(dto.messageKey);
    this.code = dto.code;
    this.retryable = dto.retryable;
    this.operationId = dto.operationId;
    this.field = dto.field;
  }
}

function isErrorDto(value: unknown): value is IpcErrorDto {
  return typeof value === "object" && value !== null && "code" in value && "retryable" in value;
}

/** Invoke one typed command, unwrapping the IpcResult envelope. */
export async function bridgeCall<TArgs extends unknown[], TResult>(
  command: keyof BridgeCommandMap,
  ...args: TArgs
): Promise<TResult> {
  // arg0 is the payload object (or undefined for commands taking none).
  const payload: unknown = args.length > 0 ? (args[0] as unknown) : {};
  const result: unknown = await invoke(command as string, payload as Record<string, unknown>);
  if (isErrorDto(result)) {
    throw new BridgeError(result);
  }
  return result as TResult;
}

/** The custom URI scheme that serves cover art (design §16).
 *  The desktop registers it and the CSP grants it to `img-src`/`media-src`
 *  only — never to scripts or fetches. */
export const COVER_SCHEME = "cover";

/**
 * Turn an opaque cover-asset key into a URL the WebView can load.
 *
 * This is deliberately the *only* place that composes an asset URL, next to the
 * only place that calls `invoke`/`listen`: no component has to know the scheme,
 * and the key stays opaque — it is the cover cache's `cv1-…` identifier, never a
 * filesystem path (the protocol re-validates it on the way in).
 */
export function assetUrl(key: string): string {
  return convertFileSrc(key, COVER_SCHEME);
}

/** Subscribe to an event stream, returning the de-registration handle. */
export async function subscribe<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<T>(event, (eventPayload) => handler(eventPayload.payload));
}

/** A tiny typed facade used by the app's store layer. */
export const bridge = {
  call: bridgeCall,
  subscribe,
  assetUrl,
};

export type { UnlistenFn };
