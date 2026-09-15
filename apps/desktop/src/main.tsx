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

// Subscribe to the desktop player snapshot stream (task 11.1): the store is
// driven by the authoritative Rust snapshot event, not by local fabrication.
void startPlayerEvents();

// Cold-start playback (task 8.9 + 默认态): restore the last locally-persisted
// session (哪个歌单的哪首歌 / 播放模式 / 音量) paused, or — with nothing
// persisted — prime the first song of 全部歌曲 into the 播放控制栏. Never
// makes a sound on its own; runs after the snapshot subscription so the
// restored state reaches the store. Failures are silent: an empty bar is an
// acceptable degradation, a broken boot is not.
void bridge.call("restore_playback_session", {}).catch(() => undefined);

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
