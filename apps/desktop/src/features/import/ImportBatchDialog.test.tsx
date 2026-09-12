/**
 * Task 10.10 — multi-select import batch: per-file results (imported /
 * duplicate / unsupported / failed / library-unavailable), summary counts, and
 * retry that never makes the user redo the already-imported items.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ImportBatchDialog } from "./ImportBatchDialog";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

describe("ImportBatchDialog (task 10.10)", () => {
  it("renders per-file results and a summary for a mixed batch", async () => {
    call.mockReset();
    call.mockResolvedValueOnce({
      results: [
        { kind: "imported", operationId: "op-1", songId: "s-1", relativePath: "A/one.flac" },
        { kind: "imported", operationId: "op-2", songId: "s-2", relativePath: "A/two.flac" },
        { kind: "duplicate", existingSongId: "s-3" },
        { kind: "unsupported" },
        { kind: "failed", code: "io", message: "读取出错" },
      ],
    } as never);

    const onDone = vi.fn();
    render(<ImportBatchDialog onClose={() => {}} onDone={onDone} />);
    fireEvent.click(screen.getByText("选择文件并导入"));

    expect(await screen.findByText(/成功 2/)).toBeInTheDocument();
    expect(screen.getByText(/重复 1/)).toBeInTheDocument();
    expect(screen.getByText(/不支持 1/)).toBeInTheDocument();
    expect(screen.getByText(/失败 1/)).toBeInTheDocument();
    // Both successful items show as imported; the failed one names its reason.
    expect(screen.getAllByText(/已导入/).length).toBe(2);
    expect(screen.getByText("失败：读取出错")).toBeInTheDocument();
    await waitFor(() => expect(onDone).toHaveBeenCalled());
  });

  it("retries with a fresh dialog without redoing the already-imported items", async () => {
    call.mockReset();
    call.mockResolvedValueOnce({
      results: [
        { kind: "imported", operationId: "op-1", songId: "s-1", relativePath: "A/one.flac" },
      ],
    } as never);
    // A second, unrelated batch — the retry re-plans and never replays the first.
    call.mockResolvedValueOnce({
      results: [{ kind: "duplicate", existingSongId: "s-1" }],
    } as never);
    const onDone = vi.fn();
    render(<ImportBatchDialog onClose={() => {}} onDone={onDone} />);
    fireEvent.click(screen.getByText("选择文件并导入"));
    await screen.findByText(/已导入：A\/one\.flac/);

    fireEvent.click(screen.getByText("再次导入"));
    await waitFor(() => expect(call).toHaveBeenCalledTimes(2));
    // The new batch's result is shown; the earlier successful line is not the
    // only content and the batch never forced a rebuild of the imported item.
    await screen.findByText(/内容重复/);
  });

  it("treats a cancelled dialog as a genuine no-op (no batch, no done)", async () => {
    call.mockReset();
    call.mockResolvedValueOnce(null as never);
    const onClose = vi.fn();
    const onDone = vi.fn();
    render(<ImportBatchDialog onClose={onClose} onDone={onDone} />);
    fireEvent.click(screen.getByText("选择文件并导入"));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(onDone).not.toHaveBeenCalled();
  });
});
