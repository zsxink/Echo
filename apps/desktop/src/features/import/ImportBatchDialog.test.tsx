/**
 * Task 10.10 — multi-select import batch: per-file results (imported /
 * duplicate / skipped / failed / library-unavailable), summary counts, and
 * retry that never makes the user redo the already-imported items.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ImportFailureDialog } from "./ImportBatchDialog";

describe("ImportFailureDialog", () => {
  it("shows only failure entries and closes", () => {
    const onClose = vi.fn();
    render(
      <ImportFailureDialog
        onClose={onClose}
        results={[{ kind: "failed", code: "io", message: "读取出错" }]}
      />,
    );
    expect(screen.getByText("导入失败")).toBeInTheDocument();
    expect(screen.queryByText(/已导入/)).not.toBeInTheDocument();
    expect(screen.getByText("失败：读取出错")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("import-close"));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("renders a library-unavailable failure", () => {
    render(
      <ImportFailureDialog onClose={() => undefined} results={[{ kind: "libraryUnavailable" }]} />,
    );
    expect(screen.getByText("音乐库不可用，未开始导入。")).toBeInTheDocument();
  });

  it("shows an imported audio file separately from its failed lyrics sidecar", () => {
    render(
      <ImportFailureDialog
        onClose={() => undefined}
        results={[
          {
            kind: "imported",
            operationId: "lyrics-operation",
            songId: "lyrics-song",
            relativePath: "media/晴天.flac",
            renamed: false,
            lyrics: { kind: "failed", code: "lyrics_publish", message: "歌词目录不可写" },
          },
        ]}
      />,
    );
    expect(screen.getByRole("heading", { name: "导入结果" })).toBeInTheDocument();
    expect(screen.getByText("已导入：media/晴天.flac")).toBeInTheDocument();
    expect(screen.getByText("音频已导入；歌词未导入：歌词目录不可写")).toBeInTheDocument();
  });

  it("shows saved-pending as saved and preserves the recovery identity", () => {
    render(
      <ImportFailureDialog
        hasPending
        onClose={() => undefined}
        results={[
          {
            kind: "savedPending",
            operationId: "original-operation",
            songId: "original-song",
            relativePath: "media/歌曲.flac",
            renamed: false,
            lyrics: { kind: "none" },
            code: "library_saved_pending_projection",
            message: "library.savedPendingProjection",
          },
          { kind: "failed", code: "io", message: "另一个文件未保存" },
        ]}
      />,
    );
    expect(screen.getByRole("heading", { name: "导入结果" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "导入失败" })).not.toBeInTheDocument();
    expect(screen.getByText("已保存：media/歌曲.flac")).toBeInTheDocument();
    expect(screen.getByText(/无需再次提交/)).toBeInTheDocument();
    expect(screen.getByText("恢复操作：original-operation")).toBeInTheDocument();
    expect(screen.getByText("失败：另一个文件未保存")).toBeInTheDocument();
    expect(screen.queryByText("library.savedPendingProjection")).not.toBeInTheDocument();
  });
});
