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

import type { IpcCommandResultMap, IpcErrorDto } from "../ipc/ipc-types.generated";

type EmptyArgs = [args?: Record<string, never>];

/**
 * The transport argument contract remains here because it describes the
 * JavaScript/Tauri invocation shape. Return values deliberately do not: they
 * are projected from Rust's generated `IpcCommandResultMap` below.
 */
interface BridgeCommandArguments {
  get_bootstrap_state: EmptyArgs;
  library_status: EmptyArgs;
  all_songs: [args: { sort: string; cursor?: string | null; limit: number }];
  search: [
    args: {
      query: string;
      inFavorites: boolean;
      sort: string;
      cursor?: string | null;
      limit: number;
    },
  ];
  favorites: [args: { sort: string; cursor?: string | null; limit: number }];
  recent: [args: { query: string }];
  library_counts: EmptyArgs;
  playlists: EmptyArgs;
  playlist_members: [args: { playlistId: string }];
  song_detail: [args: { songId: string }];
  song_cover_keys: [args: { songIds: string[] }];
  set_favorite: [args: { songId: string; favorite: boolean }];
  create_playlist: [args: { root: string; name: string }];
  rename_playlist: [args: { id: string; name: string }];
  set_playlist_cover: [
    args: {
      id: string;
      bytes: number[] | null;
      mime?: string | null;
    },
  ];
  delete_playlist: [args: { id: string }];
  add_to_playlists: [args: { song: string; targets: string[] }];
  remove_playlist_song: [args: { playlist: string; song: string }];
  delete_song: [args: { root: string; song: string }];
  undo_delete: [args: { root: string; operation: string }];
  choose_library_root: EmptyArgs;
  choose_and_import_files: EmptyArgs;
  reveal_song: [args: { songId: string }];
  start_scan: [args: { root: string }];
  cancel_scan: [args: { root: string }];
  set_theme: [args: { theme: string }];
  set_close_behavior: [args: { behavior: string }];
  get_close_behavior: EmptyArgs;
  play_playlist_context: [args: { playlist: string; selectedSong: string }];
  play_library_context: [
    args: {
      view: "all" | "recent" | "favorites";
      query: string;
      sort: string;
      selectedSong: string;
    },
  ];
  restore_playback_session: EmptyArgs;
  file_open_frontend_ready: EmptyArgs;
  play_temporary_file: [args: { path: string; displayName: string }];
  import_current_temporary_file: EmptyArgs;
  player_control: [args: { action: string }];
  queue_command: [args: { command: string; songId?: string; entryId?: string }];
  set_volume: [args: { volume: number }];
  toggle_mute: EmptyArgs;
  seek: [args: { position: number }];
  get_lyrics: [args: { songId: string }];
}

type AssertNever<T extends never> = T;
type CommandArguments<C extends keyof IpcCommandResultMap> =
  // The two assertions deliberately fail TypeScript compilation if either the
  // Rust generated command table or the hand-written Tauri payload table moves
  // without the other. They live in this used type so `noUnusedLocals` also
  // keeps the drift gate live.
  [
    AssertNever<Exclude<keyof IpcCommandResultMap, keyof BridgeCommandArguments>>,
    AssertNever<Exclude<keyof BridgeCommandArguments, keyof IpcCommandResultMap>>,
  ] extends [never, never]
    ? C extends keyof BridgeCommandArguments
      ? BridgeCommandArguments[C]
      : never
    : never;

/** A typed command name → signature with generated, concrete return values. */
export type BridgeCommandMap = {
  [C in keyof IpcCommandResultMap]: (...args: CommandArguments<C>) => IpcCommandResultMap[C];
};

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
export async function bridgeCall<C extends keyof BridgeCommandMap>(
  command: C,
  ...args: Parameters<BridgeCommandMap[C]>
): Promise<ReturnType<BridgeCommandMap[C]>> {
  // arg0 is the payload object (or undefined for commands taking none).
  const payload: unknown = args.length > 0 ? (args[0] as unknown) : {};
  const result: unknown = await invoke(command as string, payload as Record<string, unknown>);
  if (isErrorDto(result)) {
    throw new BridgeError(result);
  }
  return result as ReturnType<BridgeCommandMap[C]>;
}

/** A failure that an intentional non-blocking invocation reports. */
export interface BridgeFailure {
  readonly command: keyof BridgeCommandMap;
  readonly error: unknown;
}

type BridgeFailureReporter = (failure: BridgeFailure) => void;

const defaultFailureReporter: BridgeFailureReporter = ({ command, error }) => {
  // The desktop WebView console is collected by the Tauri devtools/runtime
  // logs. Non-blocking UI work has no component-local error surface, so this
  // is the observability boundary instead of a silent rejected promise.
  console.error(`Echo bridge command failed: ${command}`, error);
};

let bridgeFailureReporter: BridgeFailureReporter = defaultFailureReporter;

/** Report an IPC failure from a call whose UI intentionally degrades in place. */
export function reportBridgeFailure(command: keyof BridgeCommandMap, error: unknown): void {
  bridgeFailureReporter({ command, error });
}

/** Test seam for asserting that intentionally non-blocking calls remain observable. */
export function setBridgeFailureReporter(reporter: BridgeFailureReporter | null): void {
  bridgeFailureReporter = reporter ?? defaultFailureReporter;
}

/**
 * Start an IPC call whose result is intentionally irrelevant.
 *
 * This is the only fire-and-forget spelling: it records rejected calls instead
 * of relying on `void bridge.call(...)`, which discards an unregistered command
 * or transport failure without a trace.
 */
export function fireAndForget<C extends keyof BridgeCommandMap>(
  command: C,
  ...args: Parameters<BridgeCommandMap[C]>
): void {
  void bridgeCall(command, ...args).catch((error: unknown) => reportBridgeFailure(command, error));
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
  fireAndForget,
  subscribe,
  assetUrl,
};

export type { UnlistenFn };
