/**
 * The mock's playback command handlers.
 *
 * Transport control (play/pause/next/volume/seek), the play-context commands
 * that start a song from a playlist or a library view, the temporary-file
 * import, and the lyrics the player surfaces. The handlers mutate the mock's
 * `E2EState`; the *events* the runtime would publish afterwards are produced
 * by the player-stream projection (`mock-bridge-player-streams.ts`).
 */
import type { E2EState } from "./mock-bridge-state";
import type { Command, Handler } from "./mock-bridge-commands";
import { emptyError } from "./mock-bridge-commands";

/** Build the playback handler table for the mock backend. */
export function buildPlaybackHandlers(state: E2EState): Partial<Record<Command, Handler>> {
  return {
    player_control: ({ action }, st) => {
      const cur = st.nowPlaying;
      if (cur) {
        if (action === "toggle") cur.playing = !cur.playing;
        // next/prev are track changes, like the runtime's auto-advance: the
        // queue entry id changes, the position restarts and the snapshot is
        // republished (PLAYER_COMMANDS) so the UI remounts per-track
        // surfaces (issue #75's e2e path). The old `position ± 10` placeholder
        // never left the song, so nothing could assert what a track change
        // does to the immersive layout.
        if ((action === "next" || action === "prev") && st.songs.length > 0) {
          const step = action === "next" ? 1 : -1;
          const index = st.songs.findIndex((s) => s.id === cur.songId);
          const nextIndex = (index + step + st.songs.length) % st.songs.length;
          const nextSong = st.songs[nextIndex] ?? st.songs[0];
          st.nowPlaying = { songId: nextSong.id, position: 0, playing: cur.playing };
        }
      }
      return st.nowPlaying;
    },
    queue_command: () => ({ ok: true }),
    // The catch-up entry point of the split streams (issue #112). The state
    // itself arrives on the events; the command only asks for it.
    player_sync: () => null,
    set_volume: () => ({ volume: 1.0 }),
    toggle_mute: () => ({ muted: true }),
    seek: ({ position }, st) => {
      if (st.nowPlaying) st.nowPlaying.position = Number(position);
      return st.nowPlaying;
    },
    play_playlist_context: ({ selectedSong }, st) => {
      // The desktop resolves the complete playlist itself. The mock only has
      // to expose the clicked entry as current for the presentation journey.
      const id = (selectedSong as string | undefined) ?? state.songs[0]?.id;
      if (!id) return emptyError("unavailable", "noLibrary", true);
      st.nowPlaying = { songId: id, position: 0, playing: true };
      return st.nowPlaying;
    },
    // The desktop-side twin of `play_context`: the app names a *view* and a
    // song, and Core resolves the whole view on its own. View membership and
    // ordering are Core's and are covered by the Rust suite, so the mock only
    // has to start the clicked song — which is what the bar assertion reads.
    play_library_context: ({ view, query, selectedSong, artistKey, albumKey }, st) => {
      const playable = state.songs.filter((s) => s.availability === "available");
      let scoped = view === "favorites" ? playable.filter((s) => s.favorite) : playable;
      if (view === "artist" && artistKey) {
        const artist = String(artistKey).toLowerCase();
        scoped = scoped.filter((s) => (s.artist ?? "").toLowerCase() === artist);
      }
      if (view === "album" && albumKey) {
        const album = String(albumKey).toLowerCase();
        scoped = scoped.filter((s) => (s.album ?? "").toLowerCase() === album);
      }
      const q = String(query ?? "").toLowerCase();
      const matched = q ? scoped.filter((s) => (s.title ?? "").toLowerCase().includes(q)) : scoped;
      const id = (selectedSong as string | undefined) ?? matched[0]?.id;
      if (!id) return emptyError("unavailable", "noLibrary", true);
      st.nowPlaying = { songId: id, position: 0, playing: true };
      return st.nowPlaying;
    },
    play_opened_files: ({ paths }) => {
      const first = Array.isArray(paths) ? String(paths[0] ?? "") : "";
      return {
        songId: "temp-song",
        title: first.split(/[\\/]/).filter(Boolean).at(-1) || "临时文件",
        isTemporary: true,
      };
    },
    import_current_temporary_file: () => ({
      kind: "imported",
      operationId: "op-temp",
      songId: "song-temp",
      relativePath: "临时导入.mp3",
      renamed: false,
      lyrics: { kind: "none" },
    }),
    get_lyrics: ({ songId }) => {
      const s = state.songs.find((x) => x.id === songId);
      if (!s) return emptyError("not_found", "songNotFound", false);
      // Timed lyrics, deliberately: 歌词专注阅读 is only reachable from the
      // lyrics area, and an always-untimed (or always-empty) mock made that
      // whole path unreachable in the browser — which is how it shipped
      // unverified. The lines carry the song title so a scenario can assert
      // that focus shows *this* song's lyrics.
      //
      // Long enough to overflow the surface on purpose. Three lines fit any
      // panel, so `手动滚动暂停 5 秒` / `回到当前行` had no reachable trigger in
      // the browser at all — the same trap as above, one layer down.
      const lines = Array.from({ length: 24 }, (_, i) => ({
        seconds: i * 10,
        text: `${s.title} 歌词第${i + 1}行`,
      }));
      return {
        source: "embedded",
        timed: true,
        lines,
        plainText: lines.map((line) => line.text).join("\n"),
        parseError: undefined,
      };
    },
  };
}
