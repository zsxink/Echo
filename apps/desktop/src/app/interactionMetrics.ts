/** Optional measurements from an input handler through native completion and
 * the next rendered frame. Disabled unless explicitly enabled by the caller. */
export type LibraryInteractionKind = "favorite" | "createPlaylist";
export type LibraryInteractionOutcome = "committed" | "savedPending" | "failed";

export interface LibraryInteractionSample {
  readonly kind: LibraryInteractionKind;
  readonly outcome: LibraryInteractionOutcome;
  readonly durationMs: number;
  readonly queueMs: number;
  readonly backendMs: number;
}

let enabled = false;
let epoch = 0;
const samples: LibraryInteractionSample[] = [];

export const libraryInteractionMetrics = {
  enable(value = true): void {
    enabled = value;
    epoch += 1;
  },
  reset(): void {
    samples.length = 0;
    epoch += 1;
  },
  get samples(): readonly LibraryInteractionSample[] {
    return samples.slice();
  },
};

declare global {
  interface Window {
    __echoMutationMetrics__?: typeof libraryInteractionMetrics;
  }
}

if (typeof window !== "undefined") {
  window.__echoMutationMetrics__ = libraryInteractionMetrics;
  enabled = new URLSearchParams(window.location.search).get("echoPerf") === "1";
}

const NO_MEASUREMENT = {
  backendStarted() {},
  backendFinished() {},
  finish(_outcome: LibraryInteractionOutcome) {},
};

export function beginLibraryInteraction(kind: LibraryInteractionKind) {
  if (!enabled) return NO_MEASUREMENT;
  const startedAt = performance.now();
  const measurementEpoch = epoch;
  let backendStartedAt = startedAt;
  let backendFinishedAt = startedAt;
  return {
    backendStarted() {
      backendStartedAt = performance.now();
    },
    backendFinished() {
      backendFinishedAt = performance.now();
    },
    finish(outcome: LibraryInteractionOutcome) {
      requestAnimationFrame(() => {
        if (!enabled || epoch !== measurementEpoch) return;
        samples.push({
          kind,
          outcome,
          durationMs: performance.now() - startedAt,
          queueMs: backendStartedAt - startedAt,
          backendMs: backendFinishedAt - backendStartedAt,
        });
        if (samples.length > 512) samples.shift();
      });
    },
  };
}
