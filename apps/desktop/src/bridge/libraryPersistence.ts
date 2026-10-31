/** User-visible persistence states. Saved operations must be recovered by ID. */
export interface SavedLibraryOperation {
  readonly operationId?: string;
  readonly stage: "recovery" | "projection";
  readonly message: string;
}

const RECOVERY_MESSAGE = "已保存到音乐库，正在等待恢复。无需再次提交这项操作。";
const PROJECTION_MESSAGE = "已保存到音乐库，正在等待更新视图。无需再次提交这项操作。";

export function libraryPersistenceMessage(messageKey: string): string | undefined {
  switch (messageKey) {
    case "library.savedPendingRecovery":
    case "library_saved_pending_recovery":
      return RECOVERY_MESSAGE;
    case "library.savedPendingProjection":
    case "library_saved_pending_projection":
      return PROJECTION_MESSAGE;
    default:
      return undefined;
  }
}

/** The structured durability verdict of an IPC failure, when it carries one. */
function outcomeStage(value: unknown): SavedLibraryOperation["stage"] | undefined {
  if (typeof value !== "object" || value === null || !("durabilityOutcome" in value)) {
    return undefined;
  }
  const outcome = (value as { durabilityOutcome?: unknown }).durabilityOutcome;
  if (typeof outcome !== "object" || outcome === null || !("kind" in outcome)) return undefined;
  const kind = (outcome as { kind?: unknown }).kind;
  if (kind === "savedPendingRecovery") return "recovery";
  if (kind === "savedPendingProjection") return "projection";
  // `notSaved` / `complete` / `recoveryPartial` are never a pending save.
  return undefined;
}

/**
 * Read the saved-pending verdict of a failed mutation.
 *
 * The stable `code` is the primary signal, but the structured
 * `durabilityOutcome` is the contract the UI branches on: a caller that has
 * already normalized the error (or a test double that only models the
 * outcome) must still be able to tell "committed but waiting" apart from
 * "never saved". Anything else — including a plain `Error` — is unsaved, so
 * an optimistic update may roll back.
 */
export function savedLibraryOperation(error: unknown): SavedLibraryOperation | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const code = "code" in error ? (error as { code?: unknown }).code : undefined;
  const stage =
    code === "library_saved_pending_recovery"
      ? "recovery"
      : code === "library_saved_pending_projection"
        ? "projection"
        : outcomeStage(error);
  if (stage === undefined) return undefined;
  const outcome = (error as { durabilityOutcome?: { operationId?: unknown } }).durabilityOutcome;
  const operationId =
    "operationId" in error && typeof error.operationId === "string"
      ? error.operationId
      : typeof outcome?.operationId === "string"
        ? outcome.operationId
        : undefined;
  return {
    stage,
    operationId,
    message: stage === "recovery" ? RECOVERY_MESSAGE : PROJECTION_MESSAGE,
  };
}

/**
 * The short user-facing line for a failed mutation: "已保存，等待更新" when the
 * library committed, otherwise the caller's own unsaved wording.
 *
 * A committed operation is never reported as a failure — the directory is the
 * authority and the projection catches up, so the copy has to say so.
 */
export function persistenceFailureMessage(error: unknown, unsavedMessage: string): string {
  const saved = savedLibraryOperation(error);
  if (!saved) return unsavedMessage;
  return saved.stage === "recovery"
    ? "已保存到音乐库，正在等待恢复。"
    : "已保存到音乐库，正在等待更新视图。";
}
