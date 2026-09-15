/**
 * Task 13.1 — browser E2E entry.
 *
 * Installs the mock Tauri bridge, then boots the *unmodified* app shell by
 * re-using the real `src/main.tsx` bootstrap (it renders `App` and wires the
 * player store). The mock lives behind `window.__TAURI_INTERNALS__` exactly
 * like a real Tauri webview, so the app cannot tell it apart. The CDP driver
 * (`run-e2e.mjs`) loads THIS bundle in Chromium and walks the PRD acceptance
 * paths (A1–A10, A12–A14) through the exposed `__echoE2E__` mock.
 */
import { installMockBridge } from "./mock-bridge";

installMockBridge();

// Boot the real app shell (same as main.tsx but guarded so this file can be a
// module entry on its own).
import { startPlayerEvents } from "../src/player/playerStore";
import { App } from "../src/app/App";
import React from "react";
import ReactDOM from "react-dom/client";
// Visual layer: the same extracted prototype stylesheet cascade main.tsx loads
// (tokens → shell → library → player → responsive → app-extras).
import "../src/styles/tokens.css";
import "../src/styles/shell.css";
import "../src/styles/library.css";
import "../src/styles/player.css";
import "../src/styles/responsive.css";
import "../src/styles/app-extras.css";
import "../src/styles/artwork.css";

const root = document.getElementById("root");
if (!root) throw new Error("Echo E2E: #root missing");
void startPlayerEvents();
ReactDOM.createRoot(root).render(
  React.createElement(React.StrictMode, null, React.createElement(App)),
);
