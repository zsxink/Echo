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
        lyrics: { kind: "none" },
      },
      {
        kind: "imported",
        operationId: "op-2",
        songId: "s2",
        relativePath: "旧名.mp3",
        renamed: true,
        lyrics: { kind: "none" },
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

  it("keeps saved-pending imports separate from failures and complete results", () => {
    const pending = {
      kind: "savedPending" as const,
      operationId: "same-operation",
      songId: "same-song",
      relativePath: "media/歌曲.flac",
      renamed: false,
      lyrics: { kind: "none" as const },
      code: "library_saved_pending_projection",
      message: "internal detail must not become a failure",
    };
    const feedback = classifyImportResults([
      pending,
      { kind: "duplicate", existingSongId: "existing" },
      { kind: "failed", code: "io", message: "未保存" },
    ]);
    expect(feedback.pending).toEqual([pending]);
    expect(feedback.nonFailures).toHaveLength(2);
    expect(feedback.failures).toEqual([{ kind: "failed", code: "io", message: "未保存" }]);
    expect(feedback.summary).toContain("1 首等待更新");
    expect(feedback.summary).not.toContain("导入完成");
  });

  it("surfaces a lyrics failure while keeping the audio import successful", () => {
    const imported = {
      kind: "imported" as const,
      operationId: "op-lyrics",
      songId: "song-lyrics",
      relativePath: "media/晴天.flac",
      renamed: false,
      lyrics: { kind: "failed" as const, code: "lyrics_io", message: "歌词目录只读" },
    };
    const feedback = classifyImportResults([imported]);

    expect(feedback.nonFailures).toEqual([imported]);
    expect(feedback.failures).toHaveLength(0);
    expect(feedback.actionResults).toEqual([imported]);
    expect(feedback.lyricsFailures).toBe(1);
    expect(feedback.summary).toContain("1 个歌词文件未导入");
  });
});
