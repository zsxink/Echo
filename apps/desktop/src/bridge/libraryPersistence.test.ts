import { describe, expect, it } from "vitest";

import { libraryPersistenceMessage, savedLibraryOperation } from "./libraryPersistence";
import { BridgeError } from "./index";

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

  it("preserves the structured durability outcome on BridgeError", () => {
    const error = new BridgeError({
      code: "library_saved_pending_recovery",
      messageKey: "library.savedPendingRecovery",
      retryable: false,
      operationId: "operation-1",
      durabilityOutcome: {
        kind: "savedPendingRecovery",
        operationId: "operation-1",
        reason: "recoveryRequired",
      },
    });
    expect(error.durabilityOutcome).toEqual({
      kind: "savedPendingRecovery",
      operationId: "operation-1",
      reason: "recoveryRequired",
    });
    expect(error.operationId).toBe("operation-1");
  });
});
