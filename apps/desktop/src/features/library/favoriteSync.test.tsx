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
import { beforeEach, describe, expect, it } from "vitest";

import { LibraryWorkspace } from "./LibraryWorkspace";
import { PlayerBar } from "../player/PlayerBar";
import { playerStore } from "../../player/playerStore";

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
