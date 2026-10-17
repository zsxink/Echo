import type { EqCurve } from "../../ipc/ipc-types.generated";

/** A flat curve — the editor's reference for the spatial/neutral display. */
export const NEUTRAL_CURVE: EqCurve = {
  gainsDb: Array(10).fill(0),
  preampMode: "auto",
  requestedPreampDb: 0,
};
