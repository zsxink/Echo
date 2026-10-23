import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it } from "vitest";
import { LibraryRecoveryStatus } from "./LibraryRecoveryStatus";

const mocks = (
  globalThis as unknown as { __echoTest: { setInvoke: (command: string, value: unknown) => void } }
).__echoTest;

describe("LibraryRecoveryStatus", () => {
  it("keeps saved data visible when catch-up fails and never resubmits an edit", async () => {
    mocks.setInvoke("library_recovery_status", {
      root: "original-root",
      directoryHeadSeq: 2,
      projectedSeq: 1,
      needsProjection: true,
      legacyUnverified: false,
      issues: [],
    });
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
    mocks.setInvoke("library_recovery_status", {
      root: "root",
      directoryHeadSeq: 2,
      projectedSeq: 2,
      needsProjection: false,
      legacyUnverified: false,
      issues: [{ target: "retained-member", reason: "missingPlaylist" }],
    });
    const first = render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByText(/部分资料待恢复/)).toBeInTheDocument();
    fireEvent.click(screen.getByText("查看待恢复资料"));
    expect(screen.getByText("缺少关联的歌单资料")).toBeInTheDocument();
    expect(screen.getByText("资料标识：retained-member")).toBeInTheDocument();
    first.unmount();
    mocks.setInvoke("library_recovery_status", {
      root: "root",
      directoryHeadSeq: 0,
      projectedSeq: 0,
      needsProjection: false,
      legacyUnverified: true,
      issues: [],
    });
    render(<LibraryRecoveryStatus root="root" scanning={false} />);
    expect(await screen.findByText(/历史资料完整性尚未验证/)).toBeInTheDocument();
    expect(screen.queryByText("已保存资料的视图已更新。")).not.toBeInTheDocument();
  });
});
