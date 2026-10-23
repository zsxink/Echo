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

export function savedLibraryOperation(error: unknown): SavedLibraryOperation | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) return undefined;
  const stage =
    error.code === "library_saved_pending_recovery"
      ? "recovery"
      : error.code === "library_saved_pending_projection"
        ? "projection"
        : undefined;
  if (stage === undefined) return undefined;
  return {
    stage,
    operationId:
      "operationId" in error && typeof error.operationId === "string"
        ? error.operationId
        : undefined,
    message: stage === "recovery" ? RECOVERY_MESSAGE : PROJECTION_MESSAGE,
  };
}
