import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import "./styles/tokens.css";
import "./app/app.css";
import "./features/library/library.css";
import "./features/player/playerbar.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Echo: #root element is missing from index.html");
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
