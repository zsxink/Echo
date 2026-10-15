/** Deterministic library states shared by browser E2E scenarios. */
import type { Theme } from "../src/ipc/ipc-types.generated";

/** Mutable backing record owned by the fake desktop library. */
export interface MockSong {
  id: string;
  title: string;
  artist: string;
  album: string;
  durationS: number;
  favorite: boolean;
  playCount: number;
  availability: "available" | "missing" | "pending-delete";
  relativePath: string;
  coverKey?: string;
}

/** A fake standalone song library the scenarios drive against. */
export interface E2EState {
  configured: boolean;
  readOnly: boolean;
  activeRoot: string;
  songs: MockSong[];
  playlists: { id: string; name: string; memberCount: number }[];
  theme: Theme;
  closeBehavior: "exit" | "background";
  importMode: "success" | "cancelled" | "mixed" | "failed";
  nowPlaying: { songId: string; position: number; playing: boolean } | null;
}

export function defaultState(): E2EState {
  return {
    configured: false,
    readOnly: false,
    activeRoot: "",
    songs: [],
    playlists: [],
    theme: "coral",
    closeBehavior: "exit",
    importMode: "success",
    nowPlaying: null,
  };
}

export function makeSong(i: number, extra: Partial<MockSong> = {}): MockSong {
  return {
    id: `song-${i}`,
    title: `歌曲 ${i}`,
    artist: `艺人 ${i % 5}`,
    album: "专辑",
    durationS: 180 + (i % 200),
    favorite: i % 3 === 0,
    playCount: i,
    availability: "available",
    relativePath: `歌曲 ${i}.mp3`,
    // Two out of three sample songs simulate embedded artwork so a scenario
    // covers both the `.cover.has-image` path and the palette placeholder
    // (design §115: a file with no embedded cover keeps the placeholder).
    coverKey: i % 3 === 0 ? undefined : `cv1-mock${i}`,
    ...extra,
  };
}

/** A readonly `SongView` reading of a mock song (the IPC DTO the UI sees). */
/** Apply a named seed onto the mock's initial library state. */
export function applySeed(state: E2EState, seed: string) {
  switch (seed) {
    case "empty": {
      // A1: no configured root, no songs.
      state.configured = false;
      state.activeRoot = "";
      state.songs = [];
      break;
    }
    case "medium": {
      // A2/A6/A7/A8: a configured medium library (a few dozen songs).
      state.configured = true;
      state.activeRoot = "/mock/library";
      state.songs = Array.from({ length: 40 }, (_, i) => makeSong(i));
      state.playlists = [{ id: "p1", name: "我的歌单", memberCount: 6 }];
      break;
    }
    case "favorites": {
      applySeed(state, "medium");
      // A6: exactly one searchable favorite.
      state.songs[3].favorite = true;
      state.songs[3].title = "歌曲 3";
      break;
    }
    case "playing": {
      applySeed(state, "medium");
      // A8/A13: a song is the now-playing item.
      state.nowPlaying = { songId: "song-3", position: 0, playing: true };
      break;
    }
    default:
      break;
  }
}
