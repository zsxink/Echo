import type { SyncStatus } from "../../ipc/ipc-types.generated";

/**
 * Commands and subscriptions may finish out of order across stop/rejoin and
 * across a library handoff, so ordering is checked per session *and* per root.
 *
 * Rules:
 * - an older session id is always stale;
 * - within one session an older event sequence is stale;
 * - a stopped session never revives into a running one;
 * - a running session never silently changes its library;
 * - a different root may only be adopted by a newer session, because the
 *   handoff that fences the old session opens a new one. Without that, a late
 *   event for the root the user just left would overwrite the new root's
 *   genuine stopped/paused/active state.
 */
export function acceptsSyncStatus(previous: SyncStatus | null, next: SyncStatus): boolean {
  if (!previous) return true;
  if (next.sessionId < previous.sessionId) return false;
  if (next.sessionId === previous.sessionId) {
    if (next.eventSequence < previous.eventSequence) return false;
    if (!previous.enabled && next.enabled) return false;
    if (previous.enabled && next.enabled && previous.libraryId !== next.libraryId) return false;
    if (previous.localRoot !== next.localRoot) return false;
  }
  return true;
}
