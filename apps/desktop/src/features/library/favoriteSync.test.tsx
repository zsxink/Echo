/**
 * Favorite sync across surfaces (regression).
 *
 * Bug: clicking 喜欢/不喜欢 updated nothing visible — the library row rendered
 * its stale `song.favorite`, and the player bar rendered the stale
 * `song_detail` heart (which is only re-fetched when the track changes). The
 * committed `SongView` that `set_favorite` returns was discarded everywhere.
 *
 * Contract under test: the committed view is broadcast, and every mounted
 * surface — the song row AND the player bar's 收藏 heart — adopts it
 * immediately, in both directions (list → bar, bar → list).
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { LibraryWorkspace } from "./LibraryWorkspace";
import { PlayerBar } from "../player";
import { playerStore } from "../../player/playerStore";
import { ToastView } from "../../app/ToastView";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke: (command: string, value: unknown) => void };
  }
).__echoTest;

const SONG = {
  id: "song-1",
  title: "晴天",
  artist: "周杰伦",
  album: "叶惠美",
  durationS: 239,
  favorite: false,
  playCount: 0,
  availability: "available" as const,
  relativePath: "晴天.flac",
};

function snapshotOf(song: typeof SONG) {
  playerStore.publish({
    state: "playing",
    position: 10,
    duration: song.durationS,
    volume: 1,
    muted: false,
    currentQueueEntryId: "entry-1",
    currentSongId: song.id,
    queueLen: 1,
    mode: "sequential",
    currentTitle: null,
    currentCanImport: false,
    queue: [
      {
        entryId: "entry-1",
        songId: song.id,
        title: null,
        isCurrent: true,
        failed: false,
        canImport: false,
        blocked: false,
        artist: null,
        durationS: null,
        coverKey: null,
      },
    ],
  });
}

function renderSurfaces() {
  return render(
    <>
      <LibraryWorkspace view="all" title="全部歌曲" root="root-1" readOnly={false} />
      <PlayerBar />
    </>,
  );
}

const rowHeart = () => screen.getByRole("button", { name: /喜欢晴天|取消喜欢晴天/ });
const barHeart = () => screen.getByRole("button", { name: /喜欢当前歌曲|取消喜欢当前歌曲/ });

beforeEach(() => {
  mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
  mocks.setInvoke("song_detail", {
    songId: SONG.id,
    relativePath: SONG.relativePath,
    title: SONG.title,
    artist: SONG.artist,
    album: SONG.album,
    durationS: SONG.durationS,
    format: "flac",
    playCount: 0,
    favorite: SONG.favorite,
    hasCover: false,
    availability: "available",
  });
  mocks.setInvoke("song_cover_keys", {});
  mocks.setInvoke("playlists", []);
  snapshotOf(SONG);
});

describe("favorite sync across the song list and the player bar", () => {
  it("adopting a toggle made in the list updates the row and the player bar at once", async () => {
    mocks.setInvoke("set_favorite", { ...SONG, favorite: true });
    renderSurfaces();

    // The row heart only exists once the library query has resolved.
    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    expect(barHeart()).toHaveAttribute("aria-pressed", "false");

    act(() => {
      fireEvent.click(rowHeart());
    });

    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "true"));
    expect(barHeart()).toHaveAttribute("aria-pressed", "true");
  });

  it("adopting a toggle made in the player bar updates the bar and the row at once", async () => {
    mocks.setInvoke("set_favorite", { ...SONG, favorite: true });
    renderSurfaces();

    await waitFor(() => expect(barHeart()).toHaveAttribute("aria-pressed", "false"));

    act(() => {
      fireEvent.click(barHeart());
    });

    await waitFor(() => expect(barHeart()).toHaveAttribute("aria-pressed", "true"));
    expect(rowHeart()).toHaveAttribute("aria-pressed", "true");
  });

  it("a rejected mutation leaves every heart authoritative-stale instead of lying", async () => {
    // No set_favorite handler → the invoke rejects → no broadcast, no patch.
    renderSurfaces();

    await waitFor(() => expect(barHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(rowHeart());
    });
    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    expect(barHeart()).toHaveAttribute("aria-pressed", "false");
  });
});

/**
 * LIB-LOC 定位当前播放歌曲 (playlist-search-locate-import 4.3).
 *
 * The library workspace's `.library-tools` owns a locate control: a playing
 * current song rolls to its row, an absent song tells the user (the list here
 * is complete), and nothing playing voices the no-playback message.
 */
describe("LibraryWorkspace — 定位正在播放的歌曲 (LIB-LOC)", () => {
  const EMPTY_SNAPSHOT = {
    state: "stopped" as const,
    position: null,
    duration: null,
    volume: 1,
    muted: false,
    currentQueueEntryId: null,
    currentSongId: null,
    queueLen: 0,
    mode: "sequential" as const,
    currentTitle: null,
    currentArtist: null,
    currentAlbum: null,
    currentCoverKey: null,
    currentLyrics: null,
    currentCanImport: false,
    queue: [],
  };

  function setCurrentSong(songId: string | null) {
    if (songId === null) {
      playerStore.publish(EMPTY_SNAPSHOT);
      return;
    }
    playerStore.publish({ ...EMPTY_SNAPSHOT, currentSongId: songId });
  }

  afterEach(() => {
    act(() => setCurrentSong(null));
  });

  /** The workspace + the shell's single toast surface (locate feedback
   *  appears there, so it must be mounted to assert on). */
  function renderWithToast() {
    return render(
      <>
        <LibraryWorkspace view="all" title="全部歌曲" root="root-1" readOnly={false} />
        <ToastView />
      </>,
    );
  }

  it("voices 当前没有正在播放的歌曲 when nothing is playing", async () => {
    mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
    setCurrentSong(null);
    renderWithToast();
    await waitFor(() => screen.getByTestId("song-row-song-1"));

    act(() => fireEvent.click(screen.getByTestId("locate-song")));

    expect(await screen.findByTestId("toast")).toHaveTextContent("当前没有正在播放的歌曲");
  });

  it("rolls to the playing song without a toast", async () => {
    mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
    setCurrentSong(SONG.id);
    renderWithToast();
    await waitFor(() => screen.getByTestId("song-row-song-1"));

    act(() => fireEvent.click(screen.getByTestId("locate-song")));

    // Found → settles without a toast; the numeric scroll is covered by
    // SongList's own tests, so here jsdom's tiny viewport clamps to 0.
    expect(screen.queryByTestId("toast")).toBeNull();
    expect(screen.getByTestId("song-list")).toHaveProperty("scrollTop", 0);
  });

  it("voices 当前歌曲不在此列表中 for a playing song outside the list", async () => {
    mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
    setCurrentSong("outside-song");
    renderWithToast();
    await waitFor(() => screen.getByTestId("song-row-song-1"));

    act(() => fireEvent.click(screen.getByTestId("locate-song")));

    expect(await screen.findByTestId("toast")).toHaveTextContent("当前歌曲不在此列表中");
  });
});
