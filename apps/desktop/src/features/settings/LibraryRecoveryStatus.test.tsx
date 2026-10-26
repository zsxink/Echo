import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";

import type { LibraryRecoveryStatusDto } from "../../ipc/ipc-types.generated";
import { LibraryRecoveryStatus } from "./LibraryRecoveryStatus";

const mocks = (
  globalThis as unknown as { __echoTest: { setInvoke: (command: string, value: unknown) => void } }
).__echoTest;

/** The backend's status DTO; `outcome` is authoritative, never inferred here. */
function status(overrides: Partial<LibraryRecoveryStatusDto> = {}): LibraryRecoveryStatusDto {
  return {
    root: "root",
    readOnly: false,
    directoryHeadSeq: 1,
    projectedSeq: 1,
    needsProjection: false,
    legacyUnverified: false,
    issues: [],
    outcome: { kind: "complete", operationId: null },
    ...overrides,
  };
}

describe("LibraryRecoveryStatus", () => {
  it("explains that saved data remains browsable when the library is read-only", async () => {
    mocks.setInvoke("playback_retry_status", []);
    mocks.setInvoke(
      "library_recovery_status",
      status({
        readOnly: true,
        outcome: { kind: "notSaved", reason: "recoveryRequired" },
      }),
    );
    render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByText(/音乐库为只读/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "更新已保存资料" })).not.toBeInTheDocument();
  });

  it("offers the same-operation catch-up when the projection lags the saved prefix", async () => {
    mocks.setInvoke("playback_retry_status", []);
    mocks.setInvoke(
      "library_recovery_status",
      status({
        directoryHeadSeq: 2,
        projectedSeq: 1,
        needsProjection: true,
        outcome: {
          kind: "savedPendingProjection",
          operationId: null,
          reason: "projectionPending",
        },
      }),
    );
    mocks.setInvoke("retry_library_projection", undefined);
    render(<LibraryRecoveryStatus root="root" scanning={false} />);

    expect(await screen.findByText(/更改已保存到音乐库，等待更新视图/)).toBeInTheDocument();
    // The retry note is the contract: re-projecting must not read as a new edit.
    const retry = screen.getByRole("button", { name: "更新已保存资料" });
    expect(retry).toHaveAccessibleDescription(/同一项更改/);
    fireEvent.click(retry);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("retry_library_projection", { root: "root" }),
    );
  });

  it("keeps saved data visible when catch-up fails and never resubmits an edit", async () => {
    mocks.setInvoke("playback_retry_status", []);
    mocks.setInvoke(
      "library_recovery_status",
      status({
        root: "original-root",
        directoryHeadSeq: 2,
        projectedSeq: 1,
        needsProjection: true,
        outcome: {
          kind: "savedPendingProjection",
          operationId: null,
          reason: "projectionPending",
        },
      }),
    );
    mocks.setInvoke("retry_library_projection", new Error("volume unavailable"));
    render(<LibraryRecoveryStatus root="original-root" scanning={false} />);
    expect(await screen.findByText(/更改已保存到音乐库/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "更新已保存资料" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("已保存资料仍在音乐库中");
    expect(invoke).toHaveBeenCalledWith("retry_library_projection", { root: "original-root" });
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "更新已保存资料" })).toBeEnabled(),
    );
  });

  it("rereads unresolved evidence after reopening and distinguishes unknown history", async () => {
    mocks.setInvoke("playback_retry_status", []);
    mocks.setInvoke(
      "library_recovery_status",
      status({
        directoryHeadSeq: 2,
        projectedSeq: 2,
        issues: [{ target: "retained-member", reason: "missingPlaylist" }],
        outcome: { kind: "recoveryPartial", reason: "unresolvedProjection" },
      }),
    );
    const first = render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByText(/部分资料待恢复/)).toBeInTheDocument();
    fireEvent.click(screen.getByText("查看待恢复资料"));
    expect(screen.getByText("缺少关联的歌单资料")).toBeInTheDocument();
    expect(screen.getByText("资料标识：retained-member")).toBeInTheDocument();
    first.unmount();
    mocks.setInvoke(
      "library_recovery_status",
      status({
        directoryHeadSeq: 0,
        projectedSeq: 0,
        legacyUnverified: true,
        outcome: { kind: "recoveryPartial", reason: "legacyUnverified" },
      }),
    );
    render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByText(/历史资料完整性尚未验证/)).toBeInTheDocument();
    expect(screen.queryByText("已保存资料的视图已更新。")).not.toBeInTheDocument();
  });

  it("reports an unreadable status without claiming the data is gone", async () => {
    mocks.setInvoke("playback_retry_status", []);
    mocks.setInvoke("library_recovery_status", new Error("disk asleep"));
    render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("资料未被删除");
    expect(screen.getByText("恢复状态尚未确认。")).toBeInTheDocument();
  });

  it("reports pending listens without interrupting anything and names the original session", async () => {
    const reads = () =>
      vi.mocked(invoke).mock.calls.filter(([command]) => command === "playback_retry_status")
        .length;
    const before = reads();
    mocks.setInvoke("library_recovery_status", status());
    mocks.setInvoke("playback_retry_status", [
      {
        operationId: "listen-1",
        song: "song-1",
        qualifiedAt: "2026-10-27T00:00:00Z",
        reason: "libraryUnavailable",
        attempts: 3,
      },
    ]);
    render(<LibraryRecoveryStatus root="root" scanning={false} />);

    const block = await screen.findByLabelText("待记录的播放统计");
    expect(within(block).getByText(/1 次播放尚未写入音乐库/)).toBeInTheDocument();
    expect(within(block).getByText(/播放不受影响/)).toBeInTheDocument();
    fireEvent.click(within(block).getByText("查看待记录的播放"));
    expect(within(block).getByText("音乐库暂不可用")).toBeInTheDocument();
    expect(within(block).getByText(/播放标识：listen-1/)).toBeInTheDocument();
    // Diagnostics never resubmit the listen they name: opening the panel is
    // exactly one read, and no mutation follows.
    expect(reads()).toBe(before + 1);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([command]) =>
          ["retry_library_operation", "set_favorite", "playback_start"].includes(command as string),
        ),
    ).toHaveLength(0);
  });

  it("stays silent when no listen is waiting — an empty queue is the normal state", async () => {
    mocks.setInvoke("library_recovery_status", status());
    mocks.setInvoke("playback_retry_status", []);
    render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByText("已保存资料的视图已更新。")).toBeInTheDocument();
    expect(screen.queryByLabelText("待记录的播放统计")).not.toBeInTheDocument();
  });

  it("keeps the refresh control reachable by keyboard and re-reads on activation", async () => {
    mocks.setInvoke("playback_retry_status", []);
    mocks.setInvoke("library_recovery_status", status());
    render(<LibraryRecoveryStatus root="root" scanning={false} />);

    const refresh = await screen.findByRole("button", { name: "刷新状态" });
    refresh.focus();
    expect(refresh).toHaveFocus();
    // A real <button> activates on Enter through the browser's own click
    // synthesis; the assertion here is that it is focusable, enabled, and
    // wired to a re-read.
    expect(refresh).toBeEnabled();
    const before = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === "library_recovery_status").length;
    fireEvent.click(refresh);
    await waitFor(() =>
      expect(
        vi.mocked(invoke).mock.calls.filter(([command]) => command === "library_recovery_status")
          .length,
      ).toBe(before + 1),
    );
  });
});
