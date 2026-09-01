/**
 * Library-status snapshot hook (task 10.3).
 *
 * Reads `library_status` (a read-only snapshot: configured / read-only /
 * unavailable / scanning / active root) and subscribes to invalidation so the
 * workspace re-renders when the root switches or availability changes. No
 * absolute path ever crosses this boundary — only the opaque active-root id.
 */

import { useCallback, useEffect, useState } from "react";

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

export function useLibraryStatus(): LibraryStatus {
  const [status, setStatus] = useState<LibraryStatus>(INITIAL);

  useEffect(() => {
    let cancelled = false;
    void bridge.call("library_status").then((value: unknown) => {
      if (!cancelled) setStatus(value as LibraryStatus);
    });
    const unlisten = subscribe("library://status", (payload: unknown) => {
      setStatus(payload as LibraryStatus);
    });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  const refresh = useCallback(() => {
    void bridge.call("library_status").then((value: unknown) => {
      setStatus(value as LibraryStatus);
    });
  }, []);

  // Expose refresh for callers (e.g. retry buttons); it is unused in the
  // current render path but wired for the retry flow (task 10.7).
  void refresh;

  return status;
}
