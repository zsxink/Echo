import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";

// Mock the Tauri runtime so component tests can run in jsdom without a real
// backend. `invoke` returns a controllable value that tests can set via
// `__setInvoke`; `listen` registers no-ops so event subscriptions are inert.
const invokeHandlers = new Map<string, unknown>();

// @ts-expect-error - we attach a test hook to the mock object itself
globalThis.__echoTest = {
  setInvoke(command: string, value: unknown) {
    invokeHandlers.set(command, value);
  },
  reset() {
    invokeHandlers.clear();
  },
};

const tauriCore = {
  invoke: vi.fn(async (command: string, _args?: unknown) => {
    if (invokeHandlers.has(command)) {
      return invokeHandlers.get(command);
    }
    // Default: `library_status` reports unconfigured (shows the choose-root
    // view); other commands reject so tests must opt into a value.
    if (command === "library_status") {
      return { configured: false, readOnly: false, unavailable: false, scanning: false };
    }
    throw new Error(`No mock for tauri command: ${command}`);
  }),
};

const tauriEvent = {
  listen: vi.fn(async () => () => {}),
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

afterEach(() => {
  // @ts-expect-error - test hook above
  globalThis.__echoTest?.reset();
});
