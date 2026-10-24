import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { applyPlatform, currentPlatform, detectPlatform } from "./platform";

/**
 * `detectPlatform` reads two globals — `documentElement.dataset.platform`
 * (the e2e pre-set seam) and `window.__TAURI_INTERNALS__` (the WebView
 * presence sniff). Each test resets both so cases stay order-independent.
 */
function setPreset(value: string | undefined): void {
  if (value === undefined) {
    delete document.documentElement.dataset.platform;
  } else {
    document.documentElement.dataset.platform = value;
  }
}

function setWebview(present: boolean): void {
  if (present) {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {},
      configurable: true,
      writable: false,
    });
  } else {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  }
}

/** Force the UA the WebView would report per platform. */
function setUserAgent(ua: string): void {
  Object.defineProperty(window.navigator, "userAgent", {
    value: ua,
    configurable: true,
  });
}

const REAL_UA = window.navigator.userAgent;

describe("platform detection", () => {
  beforeEach(() => {
    setPreset(undefined);
    setWebview(false);
    setUserAgent(REAL_UA);
  });

  afterEach(() => {
    setPreset(undefined);
    setWebview(false);
    setUserAgent(REAL_UA);
  });

  it("a pre-set dataset value wins over every sniff (e2e forcing seam)", () => {
    setWebview(false);
    setPreset("windows");
    setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    expect(detectPlatform()).toBe("windows");
  });

  it("an unknown preset falls through to the WebView sniff", () => {
    setPreset("msdos");
    expect(detectPlatform()).toBe("web");
  });

  it("no Tauri internals means the plain-web preview", () => {
    setWebview(false);
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    expect(detectPlatform()).toBe("web");
  });

  it("the WebView UA names each desktop platform", () => {
    setWebview(true);
    setUserAgent(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
    );
    expect(detectPlatform()).toBe("macos");
    setUserAgent(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36",
    );
    expect(detectPlatform()).toBe("windows");
    setUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120 Safari/537.36");
    expect(detectPlatform()).toBe("linux");
  });

  it("applyPlatform publishes the detected value on the dataset", () => {
    setWebview(false);
    setPreset("windows");
    expect(applyPlatform()).toBe("windows");
    expect(currentPlatform()).toBe("windows");
    expect(document.documentElement.dataset.platform).toBe("windows");
  });

  it("currentPlatform is undefined before applyPlatform runs", () => {
    setPreset(undefined);
    expect(currentPlatform()).toBeUndefined();
  });
});
