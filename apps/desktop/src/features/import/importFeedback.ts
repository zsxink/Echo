import type { ImportResultDto } from "../../ipc/ipc-types.generated";

export interface ImportFeedback {
  readonly nonFailures: readonly ImportResultDto[];
  readonly failures: readonly ImportResultDto[];
  readonly pending: readonly Extract<ImportResultDto, { kind: "savedPending" }>[];
  readonly actionResults: readonly ImportResultDto[];
  readonly lyricsFailures: number;
  readonly summary: string;
}

/**
 * The one UI-facing import classifier. It deliberately separates completed
 * work from actionable failures, so a mixed batch never turns successes into
 * a blocking report.
 */
export function classifyImportResults(results: readonly ImportResultDto[]): ImportFeedback {
  const nonFailures = results.filter(
    (result) =>
      result.kind === "imported" ||
      result.kind === "duplicate" ||
      result.kind === "skipped" ||
      result.kind === "savedPending",
  );
  const failures = results.filter(
    (result) => result.kind === "failed" || result.kind === "libraryUnavailable",
  );
  const imported = nonFailures.filter((result) => result.kind === "imported").length;
  const duplicates = nonFailures.filter((result) => result.kind === "duplicate").length;
  const skipped = nonFailures.filter((result) => result.kind === "skipped").length;
  const pending = results.filter((result) => result.kind === "savedPending");
  const lyricsFailures = results.filter(
    (result) =>
      (result.kind === "imported" || result.kind === "savedPending") &&
      result.lyrics.kind === "failed",
  ).length;
  const actionResults = results.filter(
    (result) =>
      result.kind === "savedPending" ||
      result.kind === "failed" ||
      result.kind === "libraryUnavailable" ||
      (result.kind === "imported" && result.lyrics.kind === "failed"),
  );
  const skippedText = skipped > 0 ? `，跳过 ${skipped} 个` : "";
  const lyricsText = lyricsFailures > 0 ? `，${lyricsFailures} 个歌词文件未导入` : "";
  return {
    nonFailures,
    failures,
    pending,
    actionResults,
    lyricsFailures,
    summary:
      pending.length > 0
        ? `音乐已保存：${pending.length} 首等待更新，已完成 ${imported} 首，重复 ${duplicates} 首${skippedText}${lyricsText}`
        : `导入完成：已导入 ${imported} 首，重复 ${duplicates} 首${skippedText}${lyricsText}`,
  };
}
