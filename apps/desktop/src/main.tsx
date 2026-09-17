import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { bridge } from "./bridge";
import { startPlayerEvents } from "./player/playerStore";
// Visual layer: the extracted prototype stylesheet (see scripts/extract-prototype-css.mjs).
import "./styles/tokens.css";
import "./styles/shell.css";
import "./styles/library.css";
import "./styles/player.css";
import "./styles/responsive.css";
// Implementation-only surfaces the prototype does not draw (status pages, real
// library availability states, paged-load banners).
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

// macOS overlay titlebar (tauri.conf.json `titleBarStyle: "Overlay"`): the
// native titlebar is hidden but the traffic lights keep floating over the
// web content's top-left corner, so the app reserves the standard 28px
// titlebar height as a safe-area inset. No-op outside the Tauri WebView
// (jsdom tests, plain `vite` browser preview) — the inset stays 0px there.
declare global {
  interface Window {
    // Tauri v2 injects this on every IPC-capable WebView.
    readonly __TAURI_INTERNALS__?: Record<string, unknown>;
  }
}
if (window.__TAURI_INTERNALS__ && /Macintosh/.test(navigator.userAgent)) {
  document.documentElement.style.setProperty("--titlebar-inset", "28px");
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
