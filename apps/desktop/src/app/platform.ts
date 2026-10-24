/**
 * Platform identification (issue #76): the single decision point for "which
 * desktop platform is this WebView running on".
 *
 * The result is published on `documentElement.dataset.platform` by
 * `applyPlatform()` at boot and becomes the runtime source of truth —
 * components and styles read the dataset (or the helper below) instead of
 * each re-sniffing `navigator.userAgent`.
 *
 * Detection order:
 *  1. A pre-set `documentElement.dataset.platform` wins. Production never
 *     sets it; the browser e2e harness does (`?platform=…`), so a scenario
 *     can force the Windows branch of platform-gated chrome on any OS.
 *  2. No `window.__TAURI_INTERNALS__` → `web`: jsdom tests and a plain
 *     `vite` browser preview. Platform-gated window chrome stays hidden and
 *     `--titlebar-inset` stays 0px, keeping the preview prototype-identical.
 *  3. Inside the Tauri WebView, the UA still names the OS (Tauri v2 does not
 *     strip it), matching the previous macOS-only sniff in `main.tsx`.
 *
 * Reading `__TAURI_INTERNALS__` for *presence* (no `.invoke` call) is the
 * environment sniff task 10.2 explicitly allows outside the bridge.
 */

/** The platforms the shell distinguishes. `web` = no Tauri WebView at all. */
export type AppPlatform = "macos" | "windows" | "linux" | "web";

const PLATFORM_VALUES: readonly AppPlatform[] = ["macos", "windows", "linux", "web"];

/** Whether a string is a publishable platform value. */
function isAppPlatform(value: string): value is AppPlatform {
  return (PLATFORM_VALUES as readonly string[]).includes(value);
}

declare global {
  interface Window {
    // Tauri v2 injects this on every IPC-capable WebView.
    readonly __TAURI_INTERNALS__?: Record<string, unknown>;
  }
}

/** Detect the current platform without publishing it (see `applyPlatform`). */
export function detectPlatform(): AppPlatform {
  const preset = document.documentElement.dataset.platform;
  if (preset !== undefined && isAppPlatform(preset)) {
    return preset;
  }
  if (!window.__TAURI_INTERNALS__) {
    return "web";
  }
  const ua = navigator.userAgent;
  if (/Macintosh/.test(ua)) return "macos";
  if (/Windows/.test(ua)) return "windows";
  return "linux";
}

/**
 * Detect the platform once at boot and publish it on
 * `documentElement.dataset.platform`. Returns the detected platform so the
 * caller can apply platform-scoped boot decisions (the titlebar inset).
 */
export function applyPlatform(): AppPlatform {
  const platform = detectPlatform();
  document.documentElement.dataset.platform = platform;
  return platform;
}

/** The published platform — `undefined` before `applyPlatform()` ran. */
export function currentPlatform(): AppPlatform | undefined {
  const value = document.documentElement.dataset.platform;
  return value !== undefined && isAppPlatform(value) ? value : undefined;
}
