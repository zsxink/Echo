/**
 * Real React components + native AppServices over a loopback HTTP transport.
 *
 * Only unrelated window/player/sync integrations use the browser E2E double.
 * Every catalog read and mutation used by this journey reaches the Rust
 * fixture's real directory transaction and SQLite database. The extra HTTP
 * transport is included in measured time; this is not a Tauri WebView result.
 */
import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "../src/app/App";
import { startPlayerEvents } from "../src/player/playerStore";
import { installMockBridge } from "./mock-bridge";

import "../src/styles/tokens.css";
import "../src/styles/shell.css";
import "../src/styles/library.css";
import "../src/styles/player.css";
import "../src/styles/responsive.css";
import "../src/styles/app-extras.css";
import "../src/styles/artwork.css";

interface Fixture {
  readonly rootId: string;
  readonly songId: string;
  readonly songCount: number;
  readonly historyOperations: number;
}

interface NativeCall {
  readonly command: string;
  readonly startedAt: number;
  readonly completedAt: number;
  readonly durationMs: number;
  readonly serviceMs: number | null;
  readonly ok: boolean;
}

interface InteractionSample {
  readonly kind: "favorite" | "createPlaylist";
  readonly durationMs: number;
  readonly calls: readonly NativeCall[];
}

interface PerfControl {
  readonly fixture: Fixture;
  readonly nativeCalls: readonly NativeCall[];
  readonly pendingNativeCalls: number;
  /** From the synchronous click to a frame with the write acknowledged, all
   * triggered native reads settled, and the expected DOM committed. */
  measure(
    kind: InteractionSample["kind"],
    action: () => void,
    settled: () => boolean,
  ): Promise<InteractionSample>;
  runFavorites(samples: number): Promise<readonly InteractionSample[]>;
  runPlaylists(samples: number): Promise<readonly InteractionSample[]>;
}

declare global {
  interface Window {
    __echoPerf__?: PerfControl;
  }
}

const NATIVE_COMMANDS = new Set([
  "get_bootstrap_state",
  "library_status",
  "all_songs",
  "favorites",
  "library_counts",
  "playlists",
  "playlist_members",
  "song_detail",
  "song_cover_keys",
  "set_favorite",
  "create_playlist",
]);

async function start() {
  const parameter = new URLSearchParams(location.search).get("backend");
  if (!parameter) throw new Error("Performance page requires ?backend=http://127.0.0.1:<port>");
  const backend = new URL(parameter);
  if (
    backend.protocol !== "http:" ||
    !["127.0.0.1", "localhost", "[::1]"].includes(backend.hostname)
  )
    throw new Error("Performance backend must be an HTTP loopback server");
  const fixtureResponse = await fetch(new URL("/fixture", backend));
  if (!fixtureResponse.ok) throw new Error("Native fixture could not be loaded");
  const fixture = (await fixtureResponse.json()) as Fixture;

  installMockBridge();
  const internals = window.__TAURI_INTERNALS__ as {
    invoke: (command: string, args: unknown) => unknown;
  };
  const fallbackInvoke = internals.invoke;
  const nativeCalls: NativeCall[] = [];
  let pendingNativeCalls = 0;
  const waitFor = (predicate: () => boolean) =>
    new Promise<void>((resolve, reject) => {
      const startedAt = performance.now();
      const inspect = () => {
        if (pendingNativeCalls === 0 && predicate()) resolve();
        else if (performance.now() - startedAt > 10_000)
          reject(new Error("Performance DOM not ready"));
        else requestAnimationFrame(inspect);
      };
      requestAnimationFrame(inspect);
    });
  internals.invoke = (command, args) => {
    if (!NATIVE_COMMANDS.has(command)) return fallbackInvoke(command, args);
    const startedAt = performance.now();
    pendingNativeCalls += 1;
    return (async () => {
      let ok = false;
      let serviceMs: number | null = null;
      try {
        const response = await fetch(new URL("/ipc", backend), {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ command, args: args ?? {} }),
        });
        const value: unknown = await response.json();
        const durationHeader = response.headers.get("X-Echo-Service-Ms");
        if (durationHeader !== null) serviceMs = Number(durationHeader);
        ok = response.ok;
        // The product bridge turns an IpcErrorDto envelope into BridgeError.
        return value;
      } finally {
        const completedAt = performance.now();
        nativeCalls.push({
          command,
          startedAt,
          completedAt,
          durationMs: completedAt - startedAt,
          serviceMs,
          ok,
        });
        pendingNativeCalls -= 1;
      }
    })();
  };

  window.__echoPerf__ = {
    fixture,
    nativeCalls,
    get pendingNativeCalls() {
      return pendingNativeCalls;
    },
    measure(kind, action, settled) {
      if (pendingNativeCalls !== 0)
        return Promise.reject(new Error("Wait for native reads to settle"));
      const index = nativeCalls.length;
      const startedAt = performance.now();
      const mutation = kind === "favorite" ? "set_favorite" : "create_playlist";
      action();
      return new Promise((resolve, reject) => {
        const inspect = () => {
          const calls = nativeCalls.slice(index);
          const write = calls.find((call) => call.command === mutation);
          const followupReadsDone =
            kind !== "createPlaylist" ||
            (calls.some((call) => call.command === "playlists" && call.ok) &&
              calls.some((call) => call.command === "playlist_members" && call.ok));
          if (write && !write.ok) {
            reject(new Error(`${mutation} failed in native backend`));
            return;
          }
          if (write && followupReadsDone && pendingNativeCalls === 0 && settled()) {
            resolve({ kind, durationMs: performance.now() - startedAt, calls });
          } else if (performance.now() - startedAt > 10_000) {
            reject(new Error(`${mutation} did not settle within 10 seconds`));
          } else {
            requestAnimationFrame(inspect);
          }
        };
        requestAnimationFrame(inspect);
      });
    },
    async runFavorites(samples) {
      const control = window.__echoPerf__!;
      const heart = () =>
        document.querySelector<HTMLButtonElement>(
          `[data-testid="song-row-${fixture.songId}"] button.favorite`,
        );
      await waitFor(() => heart() !== null);
      const results: InteractionSample[] = [];
      for (let index = 0; index < samples; index += 1) {
        const button = heart()!;
        const expected = button.getAttribute("aria-pressed") === "true" ? "false" : "true";
        results.push(
          await control.measure(
            "favorite",
            () => button.click(),
            () => heart()?.getAttribute("aria-pressed") === expected,
          ),
        );
      }
      return results;
    },
    async runPlaylists(samples) {
      const control = window.__echoPerf__!;
      const results: InteractionSample[] = [];
      const prefix = `EchoPerf-${Date.now()}`;
      for (let index = 0; index < samples; index += 1) {
        await waitFor(() => !!document.querySelector('[data-testid="create-playlist"]'));
        document.querySelector<HTMLButtonElement>('[data-testid="create-playlist"]')!.click();
        await waitFor(() => !!document.querySelector("#playlist-name-input"));
        const input = document.querySelector<HTMLInputElement>("#playlist-name-input")!;
        const name = `${prefix}-${index}`;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(
          input,
          name,
        );
        input.dispatchEvent(new Event("input", { bubbles: true }));
        await waitFor(() => input.value === name);
        results.push(
          await control.measure(
            "createPlaylist",
            () => document.querySelector<HTMLFormElement>(".playlist-name-panel")!.requestSubmit(),
            () =>
              !document.querySelector(".playlist-name-dialog") &&
              [...document.querySelectorAll(".playlist-name")].some(
                (item) => item.textContent === name,
              ) &&
              [...document.querySelectorAll("h1,h2")].some(
                (heading) => heading.textContent === name,
              ),
          ),
        );
      }
      return results;
    },
  };

  const root = document.getElementById("root");
  if (!root) throw new Error("Performance page is missing #root");
  void startPlayerEvents();
  // The production React build and all actual App components are used. Strict
  // mode's development double mount would add unrelated startup requests.
  ReactDOM.createRoot(root).render(React.createElement(App));
}

void start().catch((error: unknown) => {
  document.body.textContent = error instanceof Error ? error.message : String(error);
  console.error(error);
});
