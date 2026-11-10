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

/**
 * The last intent each song was written with, kept past the write's
 * completion.
 *
 * The count delta is measured against this value, not against the rendered
 * row: a row can still be the pre-write value when the next click arrives
 * (a context menu holds the snapshot it opened with, and a re-read that
 * raced the write can repaint the old row). Rebuilding the baseline from
 * such a row counts the same song twice — "喜欢" twice must add one
 * favorite, not two.
 *
 * The map is bounded by the number of songs the user actually toggled;
 * entries are dropped only when a *later* intent for the same song makes
 * them redundant, and the oldest entries are evicted past the cap.
 */
const intents = new Map<string, boolean>();
const MAX_TRACKED_INTENTS = 512;

function rememberIntent(songId: string, favorite: boolean): void {
  intents.delete(songId);
  intents.set(songId, favorite);
  while (intents.size > MAX_TRACKED_INTENTS) {
    const oldest = intents.keys().next();
    if (oldest.done) break;
    intents.delete(oldest.value);
  }
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
    // The baseline is this song's last known intent. It is only the row's
    // value when the song has never been written from this window; a tracked
    // intent outranks a row that may not have caught up yet.
    const baseline = intents.get(song.id) ?? song.favorite;
    lane = { confirmed: song, desired: baseline, latest: 0, tail: null };
    lanes.set(song.id, lane);
  }
  const state = lane;
  const mutation = ++state.latest;
  bumpLibraryCount("favorites", Number(favorite) - Number(state.desired));
  state.desired = favorite;
  rememberIntent(song.id, favorite);
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
          // The write never landed, so the last *known* intent for this song
          // is the durable one again. A later click must count against it,
          // not against the intent that failed.
          rememberIntent(song.id, state.confirmed.favorite);
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
  intents.clear();
}
