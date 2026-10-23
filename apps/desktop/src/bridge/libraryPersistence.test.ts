import { describe, expect, it } from "vitest";

import { libraryPersistenceMessage, savedLibraryOperation } from "./libraryPersistence";

describe("library persistence feedback", () => {
  it.each([
    ["library_saved_pending_recovery", "library.savedPendingRecovery", "recovery"],
    ["library_saved_pending_projection", "library.savedPendingProjection", "projection"],
  ])("maps %s to user text and keeps its stable operation", (code, key, stage) => {
    const message = libraryPersistenceMessage(key);
    expect(message).toContain("已保存到音乐库");
    expect(message).not.toContain("library.");
    expect(libraryPersistenceMessage(code)).toBe(message);
    const saved = savedLibraryOperation({ code, operationId: "original-operation" });
    expect(saved).toEqual({ operationId: "original-operation", stage, message });
    expect(saved?.message).toContain("无需再次提交");
  });

  it("does not treat an uncommitted failure or malformed error as saved", () => {
    for (const failure of [null, undefined, "io", { code: "io" }, new Error("failed")]) {
      expect(savedLibraryOperation(failure)).toBeUndefined();
    }
    expect(libraryPersistenceMessage("unknown.messageKey")).toBeUndefined();
  });
});
