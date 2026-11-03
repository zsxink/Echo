/**
 * The mock's library-side command handlers.
 *
 * Everything the app asks the backend about the *library*: the song catalog
 * (lists, search, favorites, counts), the artist/album catalog, playlists, the
 * per-song detail surface, the library root and its scan/import commands, and
 * the window/theme preferences. The handlers share the mock's `E2EState`, so a
 * mutation a scenario drives is visible to the next command.
 */
import type { LibraryRootStatusDto, Theme } from "../src/ipc/ipc-types.generated";
import { makeSong } from "./mock-bridge-state";
import type { E2EState, MockSong } from "./mock-bridge-state";
import type { Command, Handler } from "./mock-bridge-commands";
import { emptyError, toView } from "./mock-bridge-commands";

/** Build the library-side handler table for the mock backend. */
export function buildLibraryHandlers(state: E2EState): Partial<Record<Command, Handler>> {
  const paged = (songs: MockSong[]) => ({
    items: songs.slice(0, 100).map(toView),
    totalCount: songs.length,
    nextCursor: undefined,
    isLast: true,
  });

  return {
    library_status: (): LibraryRootStatusDto =>
      ({
        configured: state.configured,
        readOnly: state.readOnly,
        activeRoot: state.activeRoot || undefined,
        unavailable: false,
        scanning: false,
      }) as unknown as LibraryRootStatusDto,

    get_bootstrap_state: () => ({
      ready: true,
      writesAllowed: !state.readOnly,
      activeRoot: state.activeRoot || undefined,
      recoveredOperations: 0,
    }),

    all_songs: () => paged(state.songs.filter((s) => s.availability === "available")),
    search: ({ query }) => {
      const q = String(query ?? "").toLowerCase();
      if (!q) return paged(state.songs.filter((s) => s.availability === "available"));
      return paged(
        state.songs.filter(
          (s) =>
            s.availability === "available" &&
            [s.title, s.artist, s.album].filter(Boolean).some((f) => f!.toLowerCase().includes(q)),
        ),
      );
    },
    favorites: () => paged(state.songs.filter((s) => s.favorite && s.availability === "available")),
    recent: () => paged(state.songs.filter((s) => s.availability === "available").slice(0, 100)),
    catalog_collections: ({ kind, search }) => {
      const query = String(search ?? "")
        .trim()
        .toLowerCase();
      const available = state.songs.filter((song) => song.availability === "available");
      const keyOf = (value: string | undefined, fallback: string) =>
        (value?.trim() || fallback).toLowerCase();
      const groups = new Map<
        string,
        { artistKey: string; albumKey?: string; artist: string; name: string; songs: MockSong[] }
      >();
      for (const song of available) {
        const artist = song.artist.trim() || "未知艺人";
        const artistKey = keyOf(song.artist, "未知艺人");
        const album = song.album.trim() || "未知专辑";
        const albumKey = keyOf(song.album, "未知专辑");
        const artistDirectory = kind === "artist";
        const groupKey = artistDirectory ? artistKey : `${artistKey}:${albumKey}`;
        const group = groups.get(groupKey) ?? {
          artistKey,
          albumKey: artistDirectory ? undefined : albumKey,
          artist,
          name: artistDirectory ? artist : album,
          songs: [],
        };
        group.songs.push(song);
        groups.set(groupKey, group);
      }
      return [...groups.values()]
        .filter((group) => !query || `${group.name} ${group.artist}`.toLowerCase().includes(query))
        .sort((a, b) => a.name.localeCompare(b.name, "zh-CN"))
        .map((group) => ({
          kind,
          artistKey: group.artistKey,
          albumKey: group.albumKey,
          artist: group.artist,
          name: group.name,
          songCount: group.songs.length,
          coverKey: group.songs.find((song) => song.coverKey)?.coverKey,
        }));
    },
    catalog_collection_songs: ({ kind, artistKey, albumKey, search }) => {
      const query = String(search ?? "")
        .trim()
        .toLowerCase();
      const keyOf = (value: string | undefined, fallback: string) =>
        (value?.trim() || fallback).toLowerCase();
      return state.songs
        .filter((song) => song.availability === "available")
        .filter((song) => keyOf(song.artist, "未知艺人") === artistKey)
        .filter((song) => kind === "artist" || keyOf(song.album, "未知专辑") === albumKey)
        .filter(
          (song) =>
            !query ||
            [song.title, song.artist, song.album].some((value) =>
              value.toLowerCase().includes(query),
            ),
        )
        .map(toView);
    },
    set_artist_cover: () => null,
    library_counts: () => {
      // Mirrors the Core rule: the active root's available songs, with 最近添加
      // capped at its own ceiling.
      const available = state.songs.filter((s) => s.availability === "available");
      return {
        all: available.length,
        favorites: available.filter((s) => s.favorite).length,
        recent: Math.min(available.length, 100),
      };
    },

    playlists: () => state.playlists,
    playlists_for_song: () => [],
    playlist_members: ({ playlistId }) => {
      const p = state.playlists.find((x) => x.id === playlistId);
      return paged(p ? state.songs.slice(0, p.memberCount) : []);
    },

    song_detail: ({ songId }) => {
      const s = state.songs.find((x) => x.id === songId);
      if (!s) return emptyError("not_found", "songNotFound", false);
      // Read-only detail: relative path only, no absolute path.
      return {
        id: s.id,
        songId: s.id,
        hasCover: !!s.coverKey,
        title: s.title,
        artist: s.artist,
        album: s.album,
        durationS: s.durationS,
        favorite: s.favorite,
        playCount: s.playCount,
        format: "flac",
        sampleRateHz: 44_100,
        relativePath: s.relativePath,
        needsCover: false,
        coverKey: undefined,
        lyricsTitle: undefined,
        lyricsArtist: undefined,
        lyricsSource: null,
        availability: s.availability,
      };
    },

    set_favorite: ({ songId, favorite }, st) => {
      const s = st.songs.find((x) => x.id === songId);
      if (!s) return emptyError("not_found", "songNotFound", false);
      s.favorite = Boolean(favorite);
      return toView(s);
    },

    // Embedded artwork of a batch of songs (design §115). A song whose file
    // carries none is absent from the map — the list keeps the placeholder.
    song_cover_keys: ({ songIds }) => {
      const ids = (songIds as string[] | undefined) ?? [];
      const keys: Record<string, string> = {};
      for (const id of ids) {
        const song = state.songs.find((x) => x.id === id);
        if (song?.coverKey) keys[id] = song.coverKey;
      }
      return keys;
    },

    reveal_song: ({ songId }, st) => {
      const s = st.songs.find((x) => x.id === songId);
      if (!s) return emptyError("not_found", "songNotFound", false);
      // reveal returns only a relative path by security contract (7.5).
      return { songId: s.id, relativePath: s.relativePath, revealed: true };
    },

    create_playlist: ({ name }, st) => {
      const title = String(name ?? "").trim();
      if (!title || [...title].length > 40)
        return emptyError("validation", "invalidPlaylistName", false);
      const id = `playlist-${st.playlists.length + 1}`;
      st.playlists.push({ id, name: title, memberCount: 0 });
      return { id, name: title, memberCount: 0 };
    },
    rename_playlist: ({ id, name }) => {
      const p = state.playlists.find((x) => x.id === id);
      if (!p) return emptyError("not_found", "playlistNotFound", false);
      p.name = String(name);
      return p;
    },
    delete_playlist: ({ id }) => {
      state.playlists = state.playlists.filter((x) => x.id !== id);
      return { id };
    },
    add_to_playlists: ({ song, targets }) => {
      for (const t of (targets as string[]) ?? []) {
        const p = state.playlists.find((x) => x.id === t);
        if (p) p.memberCount += 1;
      }
      return { song, targets };
    },
    remove_playlist_song: ({ playlist }) => {
      const p = state.playlists.find((x) => x.id === playlist);
      if (p) p.memberCount = Math.max(0, p.memberCount - 1);
      return { playlist };
    },

    choose_library_root: (_args, st) => {
      // In the browser journey we auto-choose a root and seed a small library.
      st.configured = true;
      st.activeRoot = "/mock/library";
      st.songs = Array.from({ length: 12 }, (_, i) => makeSong(i));
      return { cancelled: false, root: st.activeRoot };
    },
    choose_and_import_files: (_args, st) => {
      if (st.importMode === "cancelled") return null;
      if (st.importMode === "failed") {
        return { results: [{ kind: "failed", code: "corrupt", message: "文件损坏" }] };
      }
      const next = st.songs.length;
      st.songs.push(makeSong(next, { title: `新导入 ${next}` }));
      const imported = {
        kind: "imported",
        operationId: `op-${next}`,
        songId: `song-${next}`,
        relativePath: `新导入 ${next}.mp3`,
        renamed: false,
        lyrics: { kind: "none" },
      };
      if (st.importMode === "mixed") {
        return {
          results: [
            imported,
            { kind: "failed", code: "corrupt", message: "文件损坏" },
            { kind: "skipped" },
          ],
        };
      }
      return { results: [imported] };
    },
    start_scan: () => ({
      generation: 1,
      cancelled: false,
      state: "Completed",
      discovered: 0,
      processed: 0,
      created: 0,
      updated: 0,
      missing: 0,
      skipped: 0,
      failed: 0,
      issues: [],
    }),
    cancel_scan: () => ({ generation: 1, cancelled: true }),

    set_theme: ({ theme }, st) => {
      st.theme = theme as Theme;
      return { theme: st.theme };
    },
    set_close_behavior: ({ behavior }, st) => {
      st.closeBehavior = behavior as "exit" | "background";
      return { behavior: st.closeBehavior };
    },
    get_close_behavior: (_args, st) => st.closeBehavior,
  };
}
