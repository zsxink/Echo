/**
 * Renderer entry for the macOS notch player window.
 *
 * Deliberately separate from `main.tsx`: this window has no shell, no library
 * views and no settings — only the notch panel. It still subscribes to the same
 * authoritative playback stream (`player://snapshot` / `player://queue`) and
 * calls the same commands, so it is a third *view* of one session, never a
 * second player.
 *
 * Styling: `tokens.css` supplies the app tokens and reset; `notch.css` is
 * generated from `docs/prototype/echo-macos-notch-player.html` and must not be
 * hand-edited (see `scripts/extract-prototype-css.mjs`).
 */

import React from "react";
import ReactDOM from "react-dom/client";

import { startPlayerEvents } from "./player/playerStore";
import { NotchPlayer } from "./features/notch/NotchPlayer";

import "./styles/tokens.css";
import "./styles/notch.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Echo: #root element is missing from notch.html");
}

// The queue projection travels on its own event and is only published when it
// changes, so the listener has to exist before anything else asks for state.
void startPlayerEvents().catch(() => undefined);

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <NotchPlayer />
  </React.StrictMode>,
);
