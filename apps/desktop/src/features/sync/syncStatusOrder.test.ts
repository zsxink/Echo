import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../../ipc/ipc-types.generated";
import { acceptsSyncStatus } from "./syncStatusOrder";

const current = {
  enabled: true,
  sessionId: 2,
  eventSequence: 5,
  libraryId: "library-a",
} as SyncStatus;

describe("sync status ordering", () => {
  it("rejects old sessions even when their event sequence is larger", () => {
    expect(acceptsSyncStatus(current, { ...current, sessionId: 1, eventSequence: 100 })).toBe(
      false,
    );
  });
  it("rejects another library inside the same session", () => {
    expect(
      acceptsSyncStatus(current, { ...current, libraryId: "library-b", eventSequence: 6 }),
    ).toBe(false);
  });
  it("accepts a new session and rejects an older reply within that session", () => {
    const next = { ...current, sessionId: 3, libraryId: "library-b", eventSequence: 1 };
    expect(acceptsSyncStatus(current, next)).toBe(true);
    expect(acceptsSyncStatus(current, { ...current, eventSequence: 4 })).toBe(false);
  });
  it("does not revive a stopped session", () => {
    expect(
      acceptsSyncStatus(
        { ...current, enabled: false, libraryId: null },
        { ...current, eventSequence: 6 },
      ),
    ).toBe(false);
  });
});
