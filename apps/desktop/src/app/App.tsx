// Application shell entry point. Real layout, theming and feature mounting are
// built in tasks 10.x; this placeholder renders only once the bridge reports
// the backend is ready, so the UI never claims a working library it may not have.

import { useState } from "react";

export function App() {
  const [bootState] = useState<string>("unavailable");

  return (
    <main data-testid="echo-shell">
      <h1>Echo</h1>
      <p data-testid="boot-state">Boot state: {bootState}</p>
    </main>
  );
}
