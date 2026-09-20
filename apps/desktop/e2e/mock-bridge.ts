/**
 * Mock Tauri bridge for the browser E2E (task 13.1).
 *
 * The desktop UI talks to the backend exclusively through
 * `@tauri-apps/api/core.invoke` (and `@tauri-apps/api/event.listen`), which in
 * a real build read `window.__TAURI_INTERNALS__`. This module installs a
 * controllable mock of those globals so the *unmodified* app can run in a
 * plain Chromium against an in-page fake backend. It implements just enough of
 * the command/event contract (the camelCase DTOs in
 * `src/ipc/ipc-types.generated.ts`) to drive the non-platform PRD acceptance
 * paths A1–A10, A12–A14 (A11 delete/undo is exercised by the Rust fault
 * matrix + the UI component tests; here we cover its UI-facing surface).
 *
 * The mock is reset per scenario via `mockBridge.reset()` and asserted through
 * the `mockBridge.spy` call log exposed on `window.__echoE2E__`.
 */

/** The subset of the generated DTOs the browser journey needs. */
import type { BridgeCommandMap } from "../src/bridge";
import type { IpcErrorDto, LibraryRootStatusDto, Theme } from "../src/ipc/ipc-types.generated";

type AnyRecord = Record<string, unknown>;

/**
 * The mock's mutable backing song record. The IPC `SongView` is a *readonly*
 * DTO by contract (the UI must not mutate it); the backend owns mutation, so
 * the mock keeps a mutable shape and hands out fresh readonly readings.
 */
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
  /**
   * Simulates artwork embedded in the audio file (design §115 内置优先).
   * `undefined` means the file carries none, which is the case the list must
   * render with the prototype's palette placeholder.
   */
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

/** Commands the app's bridge can invoke. */
type Command =
  | "library_status"
  | "get_bootstrap_state"
  | "all_songs"
  | "search"
  | "favorites"
  | "recent"
  | "library_counts"
  | "playlists"
  | "playlist_members"
  | "song_detail"
  | "song_cover_keys"
  | "set_favorite"
  | "create_playlist"
  | "rename_playlist"
  | "set_playlist_cover"
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
  | "set_volume"
  | "toggle_mute"
  | "seek"
  | "play_playlist_context"
  | "play_library_context"
  | "play_temporary_file"
  | "import_current_temporary_file"
  | "delete_song"
  | "undo_delete"
  | "restore_playback_session"
  | "file_open_frontend_ready"
  | "get_lyrics";

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
type UnnamedCommands = Exclude<keyof BridgeCommandMap, Command>;

/** `true` while the union above names every command the bridge declares; a
 *  *command name* the moment one does not, which fails the assignment. */
export const MOCK_NAMES_EVERY_COMMAND: [UnnamedCommands] extends [never] ? true : UnnamedCommands =
  true;

/** How an invoke result can be wired. */
type Handler = (args: AnyRecord, state: E2EState) => unknown;

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

function makeSong(i: number, extra: Partial<MockSong> = {}): MockSong {
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
function toView(s: MockSong): import("../src/ipc/ipc-types.generated").SongView {
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

/**
 * Commands whose completion republishes the player snapshot, exactly as the
 * real runtime does after the actor accepts a playback request.
 */
/** Commands that make the runtime republish the player snapshot.
 *
 * A command that starts playback but is missing here accepts the request and
 * tells nobody: the handler updates the mock's own state, no event leaves, and
 * the player bar stays as it was. `play_library_context` was missing on the day
 * the app switched to it from `play_context`, which is what A8 was reporting —
 * the click worked, the song never reached the bar. */
const PLAYER_COMMANDS: ReadonlySet<string> = new Set([
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
const PLAYER_SNAPSHOT_EVENT = "player://snapshot";

/**
 * Derive the UI player snapshot from the mock's playback state.
 *
 * Without this the mock accepted a playback command and told nobody: the player bar
 * stayed empty and the row playing indicator never appeared, so "clicking a song
 * plays it" was unobservable from the preview (and the A8 acceptance check
 * passed vacuously, satisfied by the library rows rather than the player bar).
 */
function snapshotOf(state: E2EState): Record<string, unknown> {
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
    currentCanImport: false,
    queue: state.songs.map((s) => ({
      entryId: `entry-${s.id}`,
      songId: s.id,
      title: null,
      isCurrent: current?.songId === s.id,
      failed: false,
      canImport: false,
    })),
  };
}

function emptyError(code: string, messageKey: string, retryable: boolean): IpcErrorDto {
  return { code, messageKey, retryable };
}

/**
 * The artwork URL the mock answers a `cover://` request with.
 *
 * The real shell registers the `cover://` URI scheme and the WebView paints the
 * asset bytes directly. A browser loaded from a plain http origin cannot resolve
 * a custom scheme, so the mock answers the *same* call with an inline SVG whose
 * colour is derived from the key: the key still round-trips through the app
 * unchanged, and both artwork paths stay observable in the preview.
 */
function coverAssetUrl(key: string): string {
  // Spread the hue around the whole wheel. The obvious `% 360` inside the fold
  // leaves every `cv1-mockN` key on the same value up to its last character
  // (the keys share a prefix), which parked every mock cover in a single
  // 10-degree band. The immersive tint is verified by *comparing* the
  // background two different covers produce, so a fixture that cannot express
  // two different covers would make that assertion vacuous.
  const hue =
    ([...key].reduce((acc, ch) => (acc * 31 + ch.charCodeAt(0)) % 1_000_003, 7) * 137) % 360;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">` +
    `<stop offset="0" stop-color="hsl(${hue} 58% 62%)"/>` +
    `<stop offset="1" stop-color="hsl(${(hue + 46) % 360} 54% 34%)"/>` +
    `</linearGradient></defs>` +
    `<rect width="64" height="64" fill="url(#g)"/>` +
    `<circle cx="32" cy="32" r="11" fill="none" stroke="rgba(255,255,255,.55)" stroke-width="1.6"/>` +
    `<circle cx="32" cy="32" r="2.6" fill="rgba(255,255,255,.82)"/>` +
    `</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

/** Build the default handler table for the mock backend.
 *
 * Partial by design: `Command` names the whole bridge contract so it can be
 * checked against the real one, while this table answers only the part the
 * browser journey drives. Anything named but not answered throws with its own
 * name — see `invoke`. */
function buildHandlers(state: E2EState): Partial<Record<Command, Handler>> {
  const paged = (songs: MockSong[]) => ({
    items: songs.slice(0, 100).map(toView),
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
    start_scan: () => ({ generation: 1, cancelled: false }),
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

    player_control: ({ action }, st) => {
      const cur = st.nowPlaying;
      if (cur) {
        if (action === "toggle") cur.playing = !cur.playing;
        if (action === "next") cur.position = Math.max(0, cur.position + 10);
        if (action === "prev") cur.position = Math.max(0, cur.position - 10);
      }
      return cur;
    },
    queue_command: () => ({ ok: true }),
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
    play_library_context: ({ view, query, selectedSong }, st) => {
      const playable = state.songs.filter((s) => s.availability === "available");
      const scoped = view === "favorites" ? playable.filter((s) => s.favorite) : playable;
      const q = String(query ?? "").toLowerCase();
      const matched = q ? scoped.filter((s) => (s.title ?? "").toLowerCase().includes(q)) : scoped;
      const id = (selectedSong as string | undefined) ?? matched[0]?.id;
      if (!id) return emptyError("unavailable", "noLibrary", true);
      st.nowPlaying = { songId: id, position: 0, playing: true };
      return st.nowPlaying;
    },
    play_temporary_file: ({ displayName }) => ({
      songId: "temp-song",
      title: displayName ?? "临时文件",
      isTemporary: true,
    }),
    import_current_temporary_file: () => ({
      kind: "imported",
      operationId: "op-temp",
      songId: "song-temp",
      relativePath: "临时导入.mp3",
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

/** The mock internals installed on `window.__TAURI_INTERNALS__`. */
export class MockBridge {
  private handlers: Partial<Record<Command, Handler>>;
  readonly state: E2EState;
  readonly calls: string[] = [];
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** Callback registry behind the mocked `transformCallback` (event handlers). */
  private callbacks = new Map<number, (event: unknown) => void>();
  private nextCallbackId = 1;

  constructor() {
    this.state = defaultState();
    this.handlers = buildHandlers(this.state);
  }

  reset() {
    Object.assign(this.state, defaultState());
    this.calls.length = 0;
    this.listeners.clear();
    this.callbacks.clear();
    this.nextCallbackId = 1;
    this.handlers = buildHandlers(this.state);
  }

  /**
   * Mirror the real `transformCallback`: park the callback under `_<id>` on the
   * page and hand back the id. `@tauri-apps/api/event` then passes that id to
   * `plugin:event|listen`, which is how the mock learns which function to call
   * when it emits.
   */
  transformCallback(callback: (event: unknown) => void): number {
    const id = this.nextCallbackId;
    this.nextCallbackId += 1;
    this.callbacks.set(id, callback);
    (window as unknown as Record<string, unknown>)[`_${id}`] = callback;
    return id;
  }

  /** Replace or append a command handler for a specific scenario. */
  stub(command: Command, fn: Handler) {
    (this.handlers as Record<string, Handler>)[command] = fn;
  }

  invoke(cmd: string, args: unknown): unknown {
    this.calls.push(cmd);
    if (cmd === "plugin:event|listen") {
      // `listen(event, handler)` registers the transformCallback id here; wire
      // it to the same event so `emit` reaches the app's subscriber.
      const { event, handler } = (args ?? {}) as { event?: string; handler?: number };
      const callback = handler === undefined ? undefined : this.callbacks.get(handler);
      if (event !== undefined && callback) {
        this.on(event, (payload) => callback({ event, id: handler, payload }));
        // The runtime publishes the current snapshot as soon as a subscriber
        // attaches, so a seeded now-playing item is on screen without a click.
        if (event === PLAYER_SNAPSHOT_EVENT) this.emit(event, snapshotOf(this.state));
      }
      return String(handler ?? this.nextCallbackId);
    }
    if (cmd === "plugin:event|unlisten") {
      return null;
    }
    const handler = this.handlers[cmd as Command];
    if (!handler) {
      // Either the app invokes a command this double has never heard of, or it
      // names one in `Command` and deliberately leaves it unmodelled. Both are
      // drift between the app and the mock; the command name is the whole of
      // what a reader needs to act on it.
      throw new Error(`mock-bridge: \`${cmd}\` has no handler`);
    }
    const result = handler((args ?? {}) as AnyRecord, this.state);
    // A playback request republishes the snapshot, as the real runtime does
    // once the actor has accepted it.
    if (PLAYER_COMMANDS.has(cmd)) {
      this.emit(PLAYER_SNAPSHOT_EVENT, snapshotOf(this.state));
    }
    return result;
  }

  emit(event: string, payload: unknown) {
    for (const fn of this.listeners.get(event) ?? []) fn(payload);
  }

  on(event: string, fn: (payload: unknown) => void) {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event)!.add(fn);
  }
}

/**
 * Install the mock on the page before the app bundle runs.
 *
 * A scenario seed may be carried in the URL query (`?seed=<name>`) so the
 * browser E2E can bootstrap a deterministic initial library state across
 * navigations/reloads (a fresh `document` always re-runs this installer, and
 * the app hydrates once). The driver picks the seed and navigates to it.
 */
export function installMockBridge(): MockBridge {
  const bridge = new MockBridge();
  const params = new URLSearchParams(window.location.search);
  const seed = params.get("seed");
  if (seed) applySeed(bridge.state, seed);
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (cmd: string, args: unknown, _options?: unknown) => bridge.invoke(cmd, args),
    transformCallback: (cb: (event: unknown) => void) => bridge.transformCallback(cb),
    convertFileSrc: (path: string, protocol = "asset") =>
      protocol === "cover" ? coverAssetUrl(path) : `${protocol}://${btoa(path)}`,
  };
  // event plugin internals used by `@tauri-apps/api/event`:
  (window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: () => {},
  };
  (window as unknown as Record<string, unknown>).__echoE2E__ = bridge;
  return bridge;
}

/** Apply a named seed onto the mock's initial library state. */
function applySeed(state: E2EState, seed: string) {
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
