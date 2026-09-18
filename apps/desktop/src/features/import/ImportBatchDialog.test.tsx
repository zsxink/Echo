/**
 * Task 10.10 — multi-select import batch: per-file results (imported /
 * duplicate / unsupported / failed / library-unavailable), summary counts, and
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
        results={[{ kind: "unsupported" }, { kind: "failed", code: "io", message: "读取出错" }]}
      />,
    );
    expect(screen.getByText("部分文件未能导入")).toBeInTheDocument();
    expect(screen.queryByText(/已导入/)).not.toBeInTheDocument();
    expect(screen.getByText("失败：读取出错")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("import-close"));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
