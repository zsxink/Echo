/**
 * The mock's player stream projection.
 *
 * The runtime publishes the player state as three separate streams (position,
 * queue, effects — issue #112); the mock mirrors that split so the browser
 * journey sees the same event names and payload shapes the app listens for.
 * This module owns the event names and the pure projections that derive a
 * stream's payload from the mock's playback state.
 */
import type { E2EState } from "./mock-bridge-state";

/**
 * Commands whose completion republishes the player snapshot, exactly as the
 * real runtime does after the actor accepts a playback request.
 *
 * A command that starts playback but is missing here accepts the request and
 * tells nobody: the handler updates the mock's own state, no event leaves, and
 * the player bar stays as it was. `play_library_context` was missing on the day
 * the app switched to it from `play_context`, which is what A8 was reporting —
 * the click worked, the song never reached the bar.
 */
export const PLAYER_COMMANDS: ReadonlySet<string> = new Set([
  "play_playlist_context",
  "play_library_context",
  "play_temporary_file",
  "player_control",
  "queue_command",
  "set_volume",
  "toggle_mute",
  "seek",
]);

/** The event the runtime publishes each player snapshot (matches the app). */
export const PLAYER_SNAPSHOT_EVENT = "player://snapshot";
/** The event the runtime publishes the queue projection on (matches the app). */
export const PLAYER_QUEUE_EVENT = "player://queue";
/** The event the runtime publishes the effects snapshot on (matches the app). */
export const PLAYER_EFFECTS_EVENT = "player://effects";

/**
 * Derive the UI player snapshot from the mock's playback state.
 *
 * Without this the mock accepted a playback command and told nobody: the player bar
 * stayed empty and the row playing indicator never appeared, so "clicking a song
 * plays it" was unobservable from the preview (and the A8 acceptance check
 * passed vacuously, satisfied by the library rows rather than the player bar).
 *
 * The snapshot carries the transport state only — the queue projection and the
 * effects snapshot travel on their own events (issue #112).
 */
export function snapshotOf(state: E2EState, queueRevision: number): Record<string, unknown> {
  const current = state.nowPlaying;
  const song = current ? state.songs.find((s) => s.id === current.songId) : undefined;
  return {
    state: current ? (current.playing ? "playing" : "paused") : "stopped",
    position: current ? current.position : null,
    duration: song ? song.durationS : null,
    volume: 1,
    muted: false,
    currentQueueEntryId: current ? `entry-${current.songId}` : null,
    currentSongId: current ? current.songId : null,
    queueLen: state.songs.length,
    mode: "sequential",
    currentTitle: song ? song.title : null,
    // The real runtime snapshot carries the meta too, so a remounting surface
    // (immersive player, player bar) paints the real artist on its first frame
    // instead of 未知艺人 until `song_detail` answers.
    currentArtist: song ? song.artist : null,
    currentAlbum: song ? song.album : null,
    currentCanImport: false,
    queueRevision,
  };
}

/**
 * Derive the queue projection from the mock's playback state.
 *
 * The real projection is built by the coordinator and published only when the
 * queue changed; the mock mirrors that by tracking the revision next to it.
 */
export function queueProjectionOf(state: E2EState, revision: number): Record<string, unknown> {
  const current = state.nowPlaying;
  return {
    revision,
    entries: state.songs.map((s) => ({
      entryId: `entry-${s.id}`,
      songId: s.id,
      title: s.title,
      isCurrent: current?.songId === s.id,
      failed: false,
      blocked: false,
      canImport: false,
      artist: s.artist,
      durationS: s.durationS,
      coverKey: s.coverKey ?? null,
    })),
  };
}

/** The signature of the projection's content: the queue changed iff it differs. */
export function queueSignatureOf(state: E2EState): string {
  const current = state.nowPlaying;
  return state.songs.map((s) => `${s.id}:${current?.songId === s.id}`).join("|");
}
