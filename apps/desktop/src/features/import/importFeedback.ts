import type { ImportResultDto } from "../../ipc/ipc-types.generated";

export interface ImportFeedback {
  readonly nonFailures: readonly ImportResultDto[];
  readonly failures: readonly ImportResultDto[];
  readonly summary: string;
}

/**
 * The one UI-facing import classifier. It deliberately separates completed
 * work from actionable failures, so a mixed batch never turns successes into
 * a blocking report.
 */
export function classifyImportResults(results: readonly ImportResultDto[]): ImportFeedback {
  const nonFailures = results.filter(
    (result) => result.kind === "imported" || result.kind === "duplicate",
  );
  const failures = results.filter(
    (result) => result.kind !== "imported" && result.kind !== "duplicate",
  );
  const imported = nonFailures.filter((result) => result.kind === "imported").length;
  const duplicates = nonFailures.filter((result) => result.kind === "duplicate").length;
  return {
    nonFailures,
    failures,
    summary: `导入完成：已导入 ${imported} 首，重复 ${duplicates} 首`,
  };
}
