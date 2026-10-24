import { ExternalStore } from "./externalStore";

/**
 * Focus-request signal for the workspace search box (issue #73).
 *
 * The macOS native menu's 视图 → 搜索（`Cmd+F`）must focus whichever search
 * input the currently routed view owns, but each view keeps its search state
 * locally and exposes no imperative handle. A shared counter is the same
 * pattern as `libraryInvalidation`: the requester bumps the epoch, and only
 * the mounted view's subscription reacts — so exactly one input focuses.
 */
const searchFocusStore = new ExternalStore(0);

/** Request that the mounted workspace search box take focus. */
export function requestSearchFocus(): void {
  searchFocusStore.update((epoch) => epoch + 1);
}

/**
 * Run `onFocus` whenever a focus request arrives. Returns the unsubscribe
 * handle; intended for a `useEffect` next to the search input's ref.
 */
export function subscribeSearchFocus(onFocus: () => void): () => void {
  return searchFocusStore.subscribe(onFocus);
}

/** Test/teardown seam for the app-owned focus signal. */
export function resetSearchFocus(): void {
  searchFocusStore.setSnapshot(0);
}
