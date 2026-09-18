import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";

import { toastStore } from "../app/toast";

// Mock the Tauri runtime so component tests can run in jsdom without a real
// backend. `invoke` returns a controllable value that tests can set via
// `setInvoke`; registering an `Error` makes that command reject.
// `listen` registers no-ops so event subscriptions are inert.
const invokeHandlers = new Map<string, unknown>();

const eventListeners = new Map<string, (payload: unknown) => void>();

// @ts-expect-error - we attach a test hook to the mock object itself
globalThis.__echoTest = {
  setInvoke(command: string, value: unknown) {
    invokeHandlers.set(command, value);
  },
  emit(event: string, payload: unknown) {
    eventListeners.get(event)?.({ payload });
  },
  reset() {
    invokeHandlers.clear();
    eventListeners.clear();
  },
};

const tauriCore = {
  invoke: vi.fn(async (command: string, _args?: unknown) => {
    if (invokeHandlers.has(command)) {
      const value = invokeHandlers.get(command);
      // An `Error` handler means "this command fails" — the way to test a
      // failing path without leaking an unhandled rejected promise.
      if (value instanceof Error) throw value;
      return value;
    }
    // Default: `library_status` reports unconfigured (shows the choose-root
    // view); other commands reject so tests must opt into a value.
    if (command === "library_status") {
      return { configured: false, readOnly: false, unavailable: false, scanning: false };
    }
    throw new Error(`No mock for tauri command: ${command}`);
  }),
  // The real shell registers the `cover://` scheme; tests only need a URL that
  // carries the opaque key, so the scheme is observable without a WebView.
  convertFileSrc: (key: string, protocol = "asset") => `${protocol}://${key}`,
};

const tauriEvent = {
  listen: vi.fn(async (event: string, handler: (payload: unknown) => void) => {
    eventListeners.set(event, handler);
    return () => eventListeners.delete(event);
  }),
};

vi.mock("@tauri-apps/api/core", () => tauriCore);
vi.mock("@tauri-apps/api/event", () => tauriEvent);

// jsdom does not implement canvas; components that measure text/widths (e.g.
// range inputs) call `getContext` during render. Stub it so those re-renders
// do not throw — measurement returns a harmless 2d context shape.
const getContext: typeof HTMLCanvasElement.prototype.getContext = ((_id: string) => {
  const ctx = { measureText: (t: string) => ({ width: (t?.length ?? 0) * 7 }) };
  return {
    ...ctx,
    fillRect: () => {},
    clearRect: () => {},
    setTransform: () => {},
    drawImage: () => {},
    createLinearGradient: () => ({ addColorStop: () => {} }),
  } as unknown as CanvasRenderingContext2D;
}) as typeof HTMLCanvasElement.prototype.getContext;
HTMLCanvasElement.prototype.getContext = getContext;

afterEach(async () => {
  // @ts-expect-error - test hook above
  globalThis.__echoTest?.reset();
  // The toast is a module-level store shared by every component (like the
  // player store), so a toast raised in one test would otherwise still be
  // mounted in the next one.
  toastStore.dismiss();
  // Same for the resolved-artwork cache: a key answered in one test must not
  // decide what the next one renders. Imported on demand because a static
  // import would pull the bridge — and with it the Tauri core this file mocks —
  // into the module graph before `vi.mock` can install the stub.
  const { invalidateCovers } = await import("../app/coverArt");
  invalidateCovers();
  // The library counts are a module-level store like the toast and cover
  // caches: a count fetched in one test would otherwise decide what the next
  // one renders.
  const { resetLibraryState } = await import("../features/library");
  resetLibraryState();
});
