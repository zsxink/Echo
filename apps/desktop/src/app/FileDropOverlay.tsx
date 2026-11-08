/**
 * Window-level file-drop feedback (issue #117 / D1).
 *
 * Pure presentation: the shell owns absolute paths and emits only a boolean
 * hover state plus a readable rejection message. This component never
 * registers with `useOverlay`, so Escape keeps closing real overlays layer by
 * layer; `pointer-events: none` keeps the titlebar drag strip, the Windows
 * caption buttons and every existing popover hittable underneath the hint.
 *
 * The visual state is a React render decision, not an animation: under
 * `prefers-reduced-motion` the stylesheet drops the transition while the
 * appear/disappear states still change.
 */

import { useEffect, useState } from "react";

import { bridge } from "../bridge";
import { error as logError } from "../logging/log";
import { notify } from "./toast";

const DROP_STATE_EVENT = "app://file-drop-state";
const DROP_REJECTION_EVENT = "app://file-drop-rejection";

interface FileDropStatePayload {
  readonly active: boolean;
}

interface FileDropRejectionPayload {
  readonly message: string;
}

export function FileDropOverlay() {
  const [active, setActive] = useState(false);

  useEffect(() => {
    let disposeState: (() => void) | undefined;
    let disposeRejection: (() => void) | undefined;
    void bridge
      .subscribe<FileDropStatePayload>(DROP_STATE_EVENT, (state) => {
        setActive(state.active === true);
      })
      .then((dispose) => {
        disposeState = dispose;
      })
      .catch(() => logError("failed to subscribe to file-drop state", "file-drop"));
    void bridge
      .subscribe<FileDropRejectionPayload>(DROP_REJECTION_EVENT, (rejection) => {
        notify({ message: rejection.message, error: true });
      })
      .then((dispose) => {
        disposeRejection = dispose;
      })
      .catch(() => logError("failed to subscribe to file-drop rejection", "file-drop"));
    return () => {
      disposeState?.();
      disposeRejection?.();
    };
  }, []);

  if (!active) return null;
  return (
    <div
      className="file-drop-overlay"
      role="status"
      aria-label="松手即播放"
      data-testid="file-drop-overlay"
      style={{ pointerEvents: "none" }}
    >
      <span className="file-drop-overlay-label">松手即播放</span>
    </div>
  );
}
