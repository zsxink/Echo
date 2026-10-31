import { bridge } from "../../bridge";
import { beginLibraryInteraction } from "../../app/interactionMetrics";
import { bumpLibraryCount } from "./libraryCounts";
import { notifySavedPending, type SavedPendingOptions } from "./savedPendingNotice";
import { publishSongUpdate, type SongUpdate } from "./songUpdates";

interface FavoriteLane {
  confirmed: SongUpdate & { readonly favorite: boolean };
  desired: boolean;
  latest: number;
  tail: Promise<void> | null;
}

const lanes = new Map<string, FavoriteLane>();

/** Apply the click immediately, then serialize writes to this song. Other
 * songs can still commit independently. A failed latest click restores the
 * last successful commit, even when previous queued clicks also failed. */
export function setSongFavorite(
  song: SongUpdate & { readonly favorite: boolean },
  favorite: boolean,
  options: SavedPendingOptions & { readonly onFailure?: () => void } = {},
): Promise<void> {
  const metric = beginLibraryInteraction("favorite");
  let lane = lanes.get(song.id);
  if (!lane) {
    lane = { confirmed: song, desired: song.favorite, latest: 0, tail: null };
    lanes.set(song.id, lane);
  }
  const state = lane;
  const mutation = ++state.latest;
  bumpLibraryCount("favorites", Number(favorite) - Number(state.desired));
  state.desired = favorite;
  publishSongUpdate({ id: song.id, favorite }, "optimistic");

  const run = async () => {
    metric.backendStarted();
    try {
      const committed = await bridge.call("set_favorite", { songId: song.id, favorite });
      metric.backendFinished();
      state.confirmed = committed;
      if (state.latest === mutation) publishSongUpdate(committed, "favorite");
      metric.finish("committed");
    } catch (error: unknown) {
      metric.backendFinished();
      if (notifySavedPending(error, options)) {
        state.confirmed = { ...state.confirmed, favorite };
        if (state.latest === mutation) publishSongUpdate(state.confirmed, "optimistic");
        metric.finish("savedPending");
      } else {
        if (state.latest === mutation) {
          bumpLibraryCount("favorites", Number(state.confirmed.favorite) - Number(state.desired));
          state.desired = state.confirmed.favorite;
          publishSongUpdate(state.confirmed, "rollback");
          options.onFailure?.();
        }
        metric.finish("failed");
      }
    } finally {
      if (state.latest === mutation && lanes.get(song.id) === state) lanes.delete(song.id);
    }
  };

  const task = state.tail ? state.tail.then(run, run) : run();
  state.tail = task;
  return task;
}

/** Invalidate old asynchronous completions along with other feature stores. */
export function resetFavoriteMutations(): void {
  for (const state of lanes.values()) state.latest += 1;
  lanes.clear();
}
