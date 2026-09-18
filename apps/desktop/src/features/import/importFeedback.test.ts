import { describe, expect, it } from "vitest";

import { classifyImportResults } from "./importFeedback";

describe("classifyImportResults (任务 4.2 单一结果分类器)", () => {
  it("treats imported, duplicate and skipped as non-failures", () => {
    const feedback = classifyImportResults([
      {
        kind: "imported",
        operationId: "op-1",
        songId: "s1",
        relativePath: "新歌.mp3",
        renamed: false,
      },
      {
        kind: "imported",
        operationId: "op-2",
        songId: "s2",
        relativePath: "旧名.mp3",
        renamed: true,
      },
      { kind: "duplicate", existingSongId: "s3" },
      { kind: "skipped" },
    ]);
    expect(feedback.nonFailures).toHaveLength(4);
    expect(feedback.failures).toHaveLength(0);
    expect(feedback.summary).toBe("导入完成：已导入 2 首，重复 1 首，跳过 1 个");
  });

  it("reports only real failures in the failure list", () => {
    const feedback = classifyImportResults([
      { kind: "failed", code: "io", message: "读取出错" },
      { kind: "libraryUnavailable" },
      { kind: "skipped" },
    ]);
    expect(feedback.failures).toHaveLength(2);
    expect(feedback.nonFailures).toHaveLength(1);
    expect(feedback.summary).toContain("跳过 1 个");
  });

  it("keeps a summary without a skip mention when nothing was skipped", () => {
    const feedback = classifyImportResults([]);
    expect(feedback.nonFailures).toHaveLength(0);
    expect(feedback.failures).toHaveLength(0);
    expect(feedback.summary).toBe("导入完成：已导入 0 首，重复 0 首");
  });
});
