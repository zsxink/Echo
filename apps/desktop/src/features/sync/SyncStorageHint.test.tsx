import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SyncStatus } from "../../ipc/ipc-types.generated";
import { lanSyncAdapter } from "./lanSyncAdapter";
import { SyncStorageHint } from "./SyncStorageHint";

describe("SyncStorageHint", () => {
  it("warns when known remaining media exceeds available capacity", async () => {
    vi.spyOn(lanSyncAdapter, "storageSpace").mockResolvedValue({
      libraryRoot: "root-1",
      availableBytes: 1024,
    });
    render(
      <SyncStorageHint
        libraryRoot="root-1"
        status={{ totalDownloadBytes: 4096, downloadedBytes: 1024 } as SyncStatus}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("当前可用空间不足");
    expect(screen.getByRole("alert")).toHaveTextContent("尚需接收 3.0 KB");
    expect(screen.getByRole("alert")).toHaveTextContent("额外空间");
  });

  it("explains unknown incoming size before pairing without claiming sufficient space", async () => {
    vi.spyOn(lanSyncAdapter, "storageSpace").mockResolvedValue({
      libraryRoot: "root-1",
      availableBytes: 1024 ** 3,
    });
    render(<SyncStorageHint libraryRoot="root-1" status={null} />);
    expect(await screen.findByText(/当前音乐目录所在磁盘可用 1.0 GB/)).toHaveTextContent(
      "主机媒体大小将在连接后统计",
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("reports an unavailable query as unknown", async () => {
    vi.spyOn(lanSyncAdapter, "storageSpace").mockRejectedValue(new Error("volume unavailable"));
    render(<SyncStorageHint libraryRoot="root-1" status={null} />);
    expect(await screen.findByText(/无法读取音乐目录所在磁盘的可用空间/)).toBeInTheDocument();
  });

  it("discards a capacity response for a previously selected root", async () => {
    let resolveOld!: (value: { libraryRoot: string; availableBytes: number }) => void;
    vi.spyOn(lanSyncAdapter, "storageSpace")
      .mockImplementationOnce(() => new Promise((resolve) => (resolveOld = resolve)))
      .mockResolvedValue({ libraryRoot: "root-2", availableBytes: 2048 });
    const { rerender } = render(<SyncStorageHint libraryRoot="root-1" status={null} />);
    rerender(<SyncStorageHint libraryRoot="root-2" status={null} />);
    await screen.findByText(/当前音乐目录所在磁盘可用 2.0 KB/);
    await act(async () => resolveOld({ libraryRoot: "root-1", availableBytes: 1024 }));
    expect(screen.getByTestId("sync-storage-hint")).toHaveTextContent("可用 2.0 KB");
  });
});
