import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SyncConflictView } from "../../ipc/ipc-types.generated";
import { lanSyncAdapter } from "./lanSyncAdapter";
import { SyncConflictsPanel } from "./SyncConflictsPanel";

const conflict: SyncConflictView = {
  conflictId: "conflict-1",
  objectType: "playlist_name",
  objectUuid: "playlist-1",
  replacedVersion: 4,
  acceptedVersion: 6,
  replacedFields: { displayName: "旧名称" },
  acceptedFields: { displayName: "主机名称" },
  recovery: "available",
};

describe("SyncConflictsPanel", () => {
  it("shows both values and resolves by queueing a fresh local mutation", async () => {
    const list = vi
      .spyOn(lanSyncAdapter, "syncConflicts")
      .mockResolvedValueOnce([conflict])
      .mockResolvedValueOnce([]);
    const resolve = vi.spyOn(lanSyncAdapter, "resolveSyncConflict").mockResolvedValue("recovered");
    render(<SyncConflictsPanel libraryRoot="root-1" />);

    expect(await screen.findByText(/主机确认次序：4 → 6/)).toBeInTheDocument();
    expect(screen.getByText(/旧名称/)).toBeInTheDocument();
    expect(screen.getByText(/主机名称/)).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("sync-conflict-recover"));

    await waitFor(() => expect(resolve).toHaveBeenCalledWith("root-1", "conflict-1", true));
    expect(await screen.findByRole("status")).toHaveTextContent("新的本机修改");
    expect(list).toHaveBeenCalledTimes(2);
  });

  it("never offers old-value recovery after a final tombstone", async () => {
    vi.spyOn(lanSyncAdapter, "syncConflicts").mockResolvedValue([
      { ...conflict, recovery: "final_deleted" },
    ]);
    render(<SyncConflictsPanel libraryRoot="root-1" />);

    expect(await screen.findByText(/重新导入会创建新 UUID/)).toBeInTheDocument();
    expect(screen.queryByTestId("sync-conflict-recover")).not.toBeInTheDocument();
    expect(screen.getByTestId("sync-conflict-dismiss")).toHaveTextContent("保留主机当前值");
  });
});
