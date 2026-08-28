import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Vitest configuration for the Echo desktop UI tests.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
