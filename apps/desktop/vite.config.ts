import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Vite configuration for the Echo desktop UI.
// The Tauri dev/build is wired in the 1.9 Gate; vite serves the renderer UI.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    target: "es2022",
  },
});
