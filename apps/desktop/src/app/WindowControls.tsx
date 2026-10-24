/**
 * Windows caption buttons (issue #76): the self-drawn 最小化 / 最大化-还原 /
 * 关闭 trio for the undecorated main window.
 *
 * Windows is the only platform without a native titlebar — `decorations:
 * false` (tauri.windows.conf.json) — so the shell renders these in the top
 * safe-area strip (`--titlebar-inset`), mirroring what macOS gets natively
 * from the overlay titlebar's traffic lights. Every other platform (macOS,
 * Linux, plain browser preview) renders nothing: the gate is the published
 * `documentElement.dataset.platform`, set once by `applyPlatform()` at boot.
 *
 * Close follows the *native* path deliberately: `bridge.requestWindowClose()`
 * routes through CloseRequested, so the shell's CloseBehavior handler decides
 * hide-vs-exit exactly as the removed native caption did — no special case.
 * Double-click maximize stays on the shared `.titlebar-drag` strip (Tauri's
 * injected drag-region handler); these buttons only single-click.
 */

import { useEffect, useState } from "react";

import { bridge } from "../bridge";
import { Icon } from "./Icon";
import { currentPlatform } from "./platform";

/** Which caption action a button triggers (kept trivial for the dispatch). */
type CaptionAction = "minimize" | "toggleMaximize" | "close";

const ACTIONS: Record<CaptionAction, () => Promise<void>> = {
  minimize: () => bridge.minimizeWindow(),
  toggleMaximize: () => bridge.toggleMaximizeWindow(),
  close: () => bridge.requestWindowClose(),
};

/** The window state the 最大化/还原图标 reflects. */
function useWindowMaximized(): boolean {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // Read once, then track resize events: the driver fires on every
    // maximize/restore, so the glyph follows the real window state instead of
    // a locally tracked guess (native double-click on the drag strip also
    // changes it — outside this component's knowledge).
    void bridge
      .isWindowMaximized()
      .then((value) => {
        if (!disposed) setMaximized(value);
      })
      .catch(() => undefined);
    void bridge
      .onWindowResized(() => {
        void bridge
          .isWindowMaximized()
          .then((value) => {
            if (!disposed) setMaximized(value);
          })
          .catch(() => undefined);
      })
      .then((dispose) => {
        if (disposed) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  return maximized;
}

/**
 * The Windows-only caption strip. Rendered as the first child of `.app`
 * (after `.titlebar-drag`), fixed to the top-right of the window inside the
 * safe-area strip; styling lives in `.window-controls` (app-extras.css).
 */
export function WindowControls() {
  if (currentPlatform() !== "windows") {
    return null;
  }
  return <WindowControlsButtons />;
}

function WindowControlsButtons() {
  const maximized = useWindowMaximized();
  const press = (action: CaptionAction) => {
    void ACTIONS[action]().catch(() => undefined);
  };
  return (
    <div className="window-controls" data-testid="window-controls">
      <button
        type="button"
        className="window-control"
        aria-label="最小化"
        title="最小化"
        onClick={() => press("minimize")}
        data-testid="window-minimize"
      >
        <Icon name="winMinimize" />
      </button>
      <button
        type="button"
        className="window-control"
        aria-label={maximized ? "还原" : "最大化"}
        title={maximized ? "还原" : "最大化"}
        onClick={() => press("toggleMaximize")}
        data-testid="window-toggle-maximize"
      >
        <Icon name={maximized ? "winRestore" : "winMaximize"} />
      </button>
      <button
        type="button"
        className="window-control window-control-close"
        aria-label="关闭"
        title="关闭"
        onClick={() => press("close")}
        data-testid="window-close"
      >
        <Icon name="close" />
      </button>
    </div>
  );
}
