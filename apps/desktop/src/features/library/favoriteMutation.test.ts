import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { setSongFavorite } from "./favoriteMutation";
import { getLibraryCountsSnapshot, setLibraryCounts } from "./libraryCounts";
import { subscribeSongUpdates, type SongUpdate } from "./songUpdates";

const song = { id: "queued-song", favorite: false };

function deferred() {
  let resolve!: (value: unknown) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.mocked(invoke).mockClear();
  setLibraryCounts({ all: 1, favorites: 0, recent: 1, artists: 1, albums: 1 });
});

describe("favorite write coordination", () => {
  it("serializes one song and rolls the last failed click back to the preceding durable commit", async () => {
    const first = deferred();
    const second = deferred();
    vi.mocked(invoke)
      .mockImplementationOnce(() => first.promise as never)
      .mockImplementationOnce(() => second.promise as never);
    const updates: SongUpdate[] = [];
    const unsubscribe = subscribeSongUpdates((update) => updates.push(update));

    const like = setSongFavorite(song, true);
    const unlike = setSongFavorite({ ...song, favorite: true }, false);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(updates.at(-1)?.favorite).toBe(false);
    expect(getLibraryCountsSnapshot().favorites).toBe(0);

    first.resolve({ ...song, favorite: true });
    await like;
    expect(invoke).toHaveBeenNthCalledWith(2, "set_favorite", { songId: song.id, favorite: false });
    // A slow older success never repaints over the latest optimistic click.
    expect(updates.at(-1)?.favorite).toBe(false);
    second.reject(new Error("unlike was not saved"));
    await unlike;

    expect(updates.at(-1)?.favorite).toBe(true);
    expect(getLibraryCountsSnapshot().favorites).toBe(1);
    unsubscribe();
  });

  it("does not invent a favorite when both rapid clicks fail", async () => {
    const first = deferred();
    vi.mocked(invoke)
      .mockImplementationOnce(() => first.promise as never)
      .mockRejectedValueOnce(new Error("second failed"));
    const updates: SongUpdate[] = [];
    const unsubscribe = subscribeSongUpdates((update) => updates.push(update));

    const like = setSongFavorite(song, true);
    const unlike = setSongFavorite({ ...song, favorite: true }, false);
    first.reject(new Error("first failed"));
    await Promise.all([like, unlike]);

    expect(updates.at(-1)?.favorite).toBe(false);
    expect(getLibraryCountsSnapshot().favorites).toBe(0);
    unsubscribe();
  });

  it("treats a saved-pending write as the durable state for a later rollback", async () => {
    const first = deferred();
    vi.mocked(invoke)
      .mockImplementationOnce(() => first.promise as never)
      .mockRejectedValueOnce(new Error("second was not saved"));
    const updates: SongUpdate[] = [];
    const unsubscribe = subscribeSongUpdates((update) => updates.push(update));

    const like = setSongFavorite(song, true);
    const unlike = setSongFavorite({ ...song, favorite: true }, false);
    first.resolve({
      code: "library_saved_pending_projection",
      messageKey: "library.savedPendingProjection",
      retryable: true,
      operationId: "saved-operation",
    });
    await Promise.all([like, unlike]);

    expect(updates.at(-1)?.favorite).toBe(true);
    expect(getLibraryCountsSnapshot().favorites).toBe(1);
    unsubscribe();
  });

  it("does not put independent songs behind the same frontend queue", async () => {
    const first = deferred();
    const second = deferred();
    vi.mocked(invoke)
      .mockImplementationOnce(() => first.promise as never)
      .mockImplementationOnce(() => second.promise as never);

    const one = setSongFavorite(song, true);
    const two = setSongFavorite({ id: "another-song", favorite: false }, true);
    expect(invoke).toHaveBeenCalledTimes(2);
    first.resolve({ ...song, favorite: true });
    second.resolve({ id: "another-song", favorite: true });
    await Promise.all([one, two]);
  });
});
