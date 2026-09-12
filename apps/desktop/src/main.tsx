import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { startPlayerEvents } from "./player/playerStore";
import "./styles/tokens.css";
import "./app/app.css";
import "./features/library/library.css";
import "./features/player/playerbar.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Echo: #root element is missing from index.html");
}

// Subscribe to the desktop player snapshot stream (task 11.1): the store is
// driven by the authoritative Rust snapshot event, not by local fabrication.
void startPlayerEvents();

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
