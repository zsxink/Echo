import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { applyPlatform } from "./app/platform";
import { bridge } from "./bridge";
import { startPlayerEvents } from "./player/playerStore";
// Visual layer: the extracted prototype stylesheet (see scripts/extract-prototype-css.mjs).
import "./styles/tokens.css";
import "./styles/shell.css";
import "./styles/library.css";
import "./styles/player.css";
import "./styles/responsive.css";
// Implementation-only details the prototype cannot drive with live library
// data (availability states, paged-load banners, accessibility helpers).
import "./styles/app-extras.css";
import "./styles/artwork.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Echo: #root element is missing from index.html");
}

// The restore command publishes the initial authoritative snapshot. `listen`
// is asynchronous, so restoring concurrently can lose that one event and
// leave the paused bar/lyrics at zero until the user presses Play. Register the
// listener first, then request restore; a subscription failure still permits a
// safe, silent restore rather than blocking startup.
void (async () => {
  try {
    await startPlayerEvents();
  } finally {
    await bridge.call("restore_playback_session", {}).catch(() => undefined);
  }
})();

// Platform detection (issue #76): `applyPlatform()` decides once whether this
// is the macOS / Windows / Linux WebView or a plain browser preview and
// publishes the result on `documentElement.dataset.platform` — the single
// source of truth for platform-gated chrome (the `WindowControls` caption
// buttons render on Windows only).
//
// The top safe-area inset matches each platform's reserved strip: macOS keeps
// the 28px overlay-titlebar inset for the floating traffic lights; Windows
// removes the native caption entirely (`decorations: false`, tauri.windows.
// conf.json) and floats its self-drawn 28×28 controls fixed to the top-right,
// so the inset only pads the content below them — 18px, half the previous
// 36px top gap (operator feedback: the blank above 品牌行/搜索行 was too
// tall). No-op outside the Tauri
// WebView (jsdom tests, plain `vite` browser preview) — the inset stays 0px
// there and no chrome renders.
declare global {
  interface Window {
    // Tauri v2 injects this on every IPC-capable WebView.
    readonly __TAURI_INTERNALS__?: Record<string, unknown>;
  }
}
const platform = applyPlatform();
if (window.__TAURI_INTERNALS__) {
  if (platform === "macos") {
    document.documentElement.style.setProperty("--titlebar-inset", "28px");
  } else if (platform === "windows") {
    document.documentElement.style.setProperty("--titlebar-inset", "18px");
  }
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
