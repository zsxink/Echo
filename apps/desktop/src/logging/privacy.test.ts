// Frontend logging privacy tests (task 1.8).
//
// The renderer log helper must not emit full absolute paths or payload contents
// by default — same contract as the Rust core. `console` is mocked so the
// assertions inspect what would actually be written.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MockInstance } from "vitest";
import { buildLine, error, fnv1a32, info, redactMessage, warn } from "./log";

const ABS_PATH = "/Users/someone/Music/Albums/Night Drive/song.mp3";
const LYRIC = "I don't know why you don't call me anymore";
const PAYLOAD = '{"chunk":["base64","AAAA","bbbb"]}';
let spies: MockInstance[];
let captured: string[];

beforeEach(() => {
  captured = [];
  spies = [
    vi.spyOn(console, "log").mockImplementation((...args: unknown[]) => {
      captured.push(args.map(String).join(" "));
    }),
    vi.spyOn(console, "warn").mockImplementation((...args: unknown[]) => {
      captured.push(args.map(String).join(" "));
    }),
    vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
      captured.push(args.map(String).join(" "));
    }),
  ];
});

afterEach(() => {
  for (const s of spies) s.mockRestore();
});

describe("renderer logging privacy", () => {
  it("never logs the full absolute path — redacts to file name + short hash", () => {
    info(`failed to read ${ABS_PATH}`, "metadata_read");
    expect(captured).toHaveLength(1);
    const out = captured[0];
    expect(out).not.toContain(ABS_PATH);
    expect(out).not.toContain("/Users/someone/");
    expect(out).toContain("song.mp3");
    // Contains the deterministic short hash of the absolute path.
    expect(out).toContain(fnv1a32(ABS_PATH));
  });

  it("never logs lyric or payload content as plain fields — callers must hash", () => {
    // LOGGING.md: sensitive content must be pre-hashed; the helper must then
    // keep the opaque hash and never leak the raw absolute path next to it.
    error("lyrics_load failed", "lyrics_load", {
      lyricHash: fnv1a32(LYRIC),
      payloadHash: fnv1a32(PAYLOAD),
      path: ABS_PATH,
    });
    expect(captured).toHaveLength(1);
    const out = captured[0];
    expect(out).not.toContain(ABS_PATH);
    expect(out).not.toContain("/Users/someone/");
    expect(out).not.toContain(LYRIC);
    expect(out).not.toContain(PAYLOAD);
    // The opaque hashes survive.
    expect(out).toContain(fnv1a32(LYRIC));
    expect(out).toContain(fnv1a32(PAYLOAD));
  });

  it("redacts a spaced absolute path inside a structured field and keeps the tag field", () => {
    warn("tag parse", "probe", { path: ABS_PATH });
    expect(captured).toHaveLength(1);
    const out = captured[0];
    expect(out).not.toContain(ABS_PATH);
    expect(out).not.toContain("/Users/someone/");
    expect(out).not.toContain("Night Drive");
    // The redacted location survives.
    expect(out).toContain("song.mp3");
    expect(out).toContain("tag parse");
  });

  it("keeps the safe structured fields (operation, level, redacted path)", () => {
    const built = buildLine("ERROR", `read failed for ${ABS_PATH}`, "song_load");
    expect(built).toContain("level=ERROR");
    expect(built).toContain('operation="song_load"');
    expect(built).toContain("song.mp3");
    expect(built).not.toContain(ABS_PATH);
  });

  it("redactMessage is deterministic", () => {
    expect(redactMessage(ABS_PATH)).toBe(redactMessage(ABS_PATH));
  });

  it("leaves relative-looking strings and hashes untouched", () => {
    const rel = "Albums/Night Drive/song.mp3";
    expect(redactMessage(rel)).toContain(rel);
    const hash = "7a3fa9e8c91d4b2f";
    expect(redactMessage(hash)).toBe(hash);
  });
});
