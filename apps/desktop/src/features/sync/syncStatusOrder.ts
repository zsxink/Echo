import type { SyncStatus } from "../../ipc/ipc-types.generated";

/** Commands and subscriptions may finish out of order across stop/rejoin. */
export function acceptsSyncStatus(previous: SyncStatus | null, next: SyncStatus): boolean {
  if (!previous) return true;
  if (next.sessionId < previous.sessionId) return false;
  if (next.sessionId === previous.sessionId) {
    if (next.eventSequence < previous.eventSequence) return false;
    if (!previous.enabled && next.enabled) return false;
    if (previous.enabled && next.enabled && previous.libraryId !== next.libraryId) return false;
  }
  return true;
}
