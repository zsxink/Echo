/**
 * Library-status snapshot hook (task 10.3).
 *
 * Reads `library_status` (a read-only snapshot: configured / read-only /
 * unavailable / scanning / active root) and subscribes to invalidation so the
 * workspace re-renders when the root switches or availability changes. No
 * absolute path ever crosses this boundary — only the opaque active-root id.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { subscribe } from "../../bridge";

/** The UI-facing library status (mirrors `LibraryStatus` DTO). */
export interface LibraryStatus {
  readonly configured: boolean;
  readonly readOnly: boolean;
  readonly unavailable: boolean;
  readonly scanning: boolean;
  readonly activeRoot?: string;
}

const INITIAL: LibraryStatus = {
  configured: false,
  readOnly: false,
  unavailable: false,
  scanning: false,
};

export function useLibraryStatus(): LibraryStatus & { refresh: () => Promise<void> } {
  const [status, setStatus] = useState<LibraryStatus>(INITIAL);
  const revision = useRef(0);
  const invalidate = useCallback(() => ++revision.current, []);

  const refresh = useCallback(async () => {
    const request = invalidate();
    const value = (await bridge.call("library_status")) as LibraryStatus;
    if (request === revision.current) setStatus(value);
  }, [invalidate]);

  useEffect(() => {
    // A failed startup read keeps the choose/retry entry available.
    void refresh().catch(() => {});
    const unlisten = subscribe("library://status", (payload: unknown) => {
      invalidate();
      setStatus(payload as LibraryStatus);
    });
    return () => {
      invalidate();
      unlisten.then((fn) => fn());
    };
  }, [refresh, invalidate]);

  return { ...status, refresh };
}
