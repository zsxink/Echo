import { describe, expect, it } from "vitest";
import type { SyncStatus } from "../../ipc/ipc-types.generated";
import { acceptsSyncStatus } from "./syncStatusOrder";

const current = {
  enabled: true,
  sessionId: 2,
  eventSequence: 5,
  libraryId: "library-a",
  localRoot: "root-a",
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
  it("rejects a late event for the root the user just left", () => {
    expect(
      acceptsSyncStatus(current, {
        ...current,
        enabled: false,
        localRoot: "root-b",
        libraryId: null,
        eventSequence: 6,
      }),
    ).toBe(false);
  });
  it("adopts the new root's own status from the handoff's new session", () => {
    const switched = {
      enabled: false,
      role: "none",
      state: "disabled",
      sessionId: 3,
      eventSequence: 1,
      catalogChanged: false,
      libraryId: "library-b",
      localRoot: "root-b",
      address: null,
      lastError: null,
      pendingChanges: null,
      pendingUploads: null,
      pendingDownloads: null,
      uploadedBytes: null,
      totalUploadBytes: null,
      downloadedBytes: null,
      totalDownloadBytes: null,
      mediaReady: null,
      sessionIntent: "stopped",
      restoreAttempts: null,
      restoreExhausted: false,
    } as SyncStatus;
    expect(acceptsSyncStatus(current, switched)).toBe(true);
  });
  it("keeps a valid status for a newly selected root that is stopped or paused", () => {
    const previous = {
      ...current,
      enabled: false,
      role: "none",
      state: "disabled",
      sessionId: 4,
      eventSequence: 2,
      libraryId: "library-b",
      localRoot: "root-b",
      sessionIntent: "stopped",
    } as SyncStatus;
    const paused = {
      ...previous,
      state: "paused",
      eventSequence: 3,
      sessionIntent: "paused",
    } as SyncStatus;
    expect(acceptsSyncStatus(previous, paused)).toBe(true);
    expect(acceptsSyncStatus(previous, { ...previous, eventSequence: 1 })).toBe(false);
  });
});
