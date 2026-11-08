/**
 * The command contract of the browser E2E mock backend.
 *
 * The mock answers the app's bridge commands; `Command` names every command
 * the real bridge declares, and `MOCK_NAMES_EVERY_COMMAND` fails the build
 * when the app grows a command the union does not name yet. The handler
 * signature (`Handler`) and the two DTO helpers (`toView`, `emptyError`) live
 * here because every handler group builds on them.
 */
import type { BridgeCommandMap } from "../src/bridge";
import type { IpcErrorDto, SongView } from "../src/ipc/ipc-types.generated";
import type { E2EState, MockSong } from "./mock-bridge-state";

/** The loosely typed argument bag the bridge hands to a handler. */
export type AnyRecord = Record<string, unknown>;

/** Commands the app's bridge can invoke. */
export type Command =
  | "library_status"
  | "get_bootstrap_state"
  | "all_songs"
  | "search"
  | "favorites"
  | "recent"
  | "catalog_collections"
  | "catalog_collection_songs"
  | "set_artist_cover"
  | "library_counts"
  | "playlists"
  | "playlist_members"
  | "playlists_for_song"
  | "song_detail"
  | "song_cover_keys"
  | "set_favorite"
  | "create_playlist"
  | "rename_playlist"
  | "set_playlist_cover"
  | "preview_cover"
  | "delete_playlist"
  | "add_to_playlists"
  | "remove_playlist_song"
  | "choose_library_root"
  | "choose_and_import_files"
  | "reveal_song"
  | "start_scan"
  | "cancel_scan"
  | "set_theme"
  | "set_close_behavior"
  | "get_close_behavior"
  | "player_control"
  | "queue_command"
  | "player_sync"
  | "set_volume"
  | "toggle_mute"
  | "seek"
  | "report_library_view"
  | "play_playlist_context"
  | "play_library_context"
  | "play_temporary_file"
  | "import_current_temporary_file"
  | "delete_song"
  | "undo_delete"
  | "restore_playback_session"
  | "file_open_frontend_ready"
  | "get_lyrics"
  | "get_audio_effects_snapshot"
  | "select_audio_effects_preset"
  | "edit_audio_equalizer"
  | "set_audio_effects_enabled"
  | "reset_audio_effects"
  | "save_audio_effects_preset"
  | "rename_audio_effects_preset"
  | "delete_audio_effects_preset"
  | "retry_audio_effects"
  | "retry_library_operation"
  | "library_recovery_status"
  | "retry_library_projection"
  | "playback_retry_status"
  | "sync_status"
  | "sync_storage_space"
  | "sync_conflicts"
  | "resolve_sync_conflict"
  | "recover_restored_host"
  | "local_lan_addresses"
  | "host_start"
  | "host_stop"
  | "sync_leave"
  | "sync_peer_start"
  | "sync_pause"
  | "sync_resume"
  | "sync_now"
  | "sync_restore_retry"
  | "sync_authorized_peers"
  | "revoke_device"
  | "discover_hosts"
  | "create_sync_pairing_code"
  | "request_sync_pairing";

/**
 * Commands the app can invoke but this mock deliberately does not model, so the
 * list above stays a complete *name* of the bridge contract even where there is
 * no answer behind it. `invoke` throws for these, naming the command; nothing
 * reaches them silently.
 *
 * `delete_song` / `undo_delete` are covered by the Rust fault matrix and the UI
 * component tests. `restore_playback_session` is called once at boot and
 * *swallowed* by its caller (`main.tsx`), so modelling it would quietly prime
 * the player bar and break A8's "the bar starts empty on a library-only seed"
 * precondition — the browser journey deliberately starts cold.
 */

/**
 * Every command the app's bridge declares must be *named* in the union above,
 * even when it has no handler. The list is hand-written and used to drift in
 * silence: `play_library_context` was missing for exactly that reason — the app
 * switched to it, the double kept answering the retired context command, and clicking a
 * library row threw "unknown command" inside a `void bridge.call(...)` nobody
 * was watching. The browser journey went red three checks later, on assertions
 * with no visible connection to the cause.
 *
 * `never` means nothing is missing. Otherwise this becomes the name of a
 * command that has to be added above.
 */
export type UnnamedCommands = Exclude<keyof BridgeCommandMap, Command>;

/** `true` while the union above names every command the bridge declares; a
 *  *command name* the moment one does not, which fails the assignment. */
export const MOCK_NAMES_EVERY_COMMAND: [UnnamedCommands] extends [never] ? true : UnnamedCommands =
  true;

/** How an invoke result can be wired. */
export type Handler = (args: AnyRecord, state: E2EState) => unknown;

/** The IPC view of a mock song. */
export function toView(s: MockSong): SongView {
  return {
    id: s.id,
    title: s.title,
    artist: s.artist,
    album: s.album,
    durationS: s.durationS,
    favorite: s.favorite,
    playCount: s.playCount,
    availability: s.availability,
    relativePath: s.relativePath,
  };
}

/** The IPC error the mock answers a rejected command with. */
export function emptyError(code: string, messageKey: string, retryable: boolean): IpcErrorDto {
  return { code, messageKey, retryable };
}
