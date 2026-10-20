import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { lanSyncAdapter } from "../features/sync/lanSyncAdapter";
import type { SyncStatus } from "../ipc/ipc-types.generated";
import { SyncToolbar } from "./SyncToolbar";

vi.mock("../features/sync/lanSyncAdapter", () => ({
  lanSyncAdapter: {
    status: vi.fn(),
    subscribeStatus: vi.fn(),
    syncNow: vi.fn(),
    pause: vi.fn(),
    resume: vi.fn(),
  },
}));

function status(overrides: Partial<SyncStatus> = {}): SyncStatus {
  return {
    enabled: true,
    role: "peer",
    state: "connected",
    sessionId: 1,
    eventSequence: 1,
    catalogChanged: false,
    libraryId: "library-a",
    localRoot: "root-1",
    address: "192.168.1.2:50826",
    lastError: null,
    pendingChanges: 0,
    pendingUploads: 0,
    pendingDownloads: 0,
    uploadedBytes: 0,
    totalUploadBytes: 0,
    downloadedBytes: 0,
    totalDownloadBytes: 0,
    mediaReady: true,
    ...overrides,
  };
}

let publish: (next: SyncStatus) => void;
const unlisten = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(lanSyncAdapter.status).mockResolvedValue(status());
  vi.mocked(lanSyncAdapter.subscribeStatus).mockImplementation(async (handler) => {
    publish = handler;
    return unlisten;
  });
});

function openPanel() {
  fireEvent.click(screen.getByTestId("sync-button"));
}

describe("SyncToolbar", () => {
  it("reads disabled status without starting any network operation", async () => {
    vi.mocked(lanSyncAdapter.status).mockResolvedValue(
      status({ enabled: false, role: "none", state: "disabled", mediaReady: false }),
    );
    render(<SyncToolbar />);
    expect(await screen.findByRole("button", { name: "音乐库同步：同步已关闭" })).toBeVisible();
    openPanel();
    expect(screen.getByText(/在设置中选择/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "立即同步" })).not.toBeInTheDocument();
    expect(lanSyncAdapter.syncNow).not.toHaveBeenCalled();
    expect(lanSyncAdapter.resume).not.toHaveBeenCalled();
  });

  it("requires media publication and empty queues before reporting synced", async () => {
    vi.mocked(lanSyncAdapter.status).mockResolvedValue(status({ mediaReady: false }));
    render(<SyncToolbar />);
    await screen.findByRole("button", { name: "音乐库同步：等待媒体就绪" });
    openPanel();
    expect(screen.getByText("媒体全部就绪后才会显示“已同步”。")).toBeInTheDocument();
    act(() => publish(status({ eventSequence: 2, pendingChanges: 3, mediaReady: true })));
    expect(screen.getByRole("button", { name: "音乐库同步：仍有待同步" })).toBeVisible();
    expect(screen.getByText("待确认修改：3 项")).toBeInTheDocument();
    act(() => publish(status({ eventSequence: 3 })));
    expect(screen.getByRole("button", { name: "音乐库同步：已同步" })).toBeVisible();
  });

  it("updates queue counts and both byte progress values from native events", async () => {
    render(<SyncToolbar />);
    await screen.findByRole("button", { name: "音乐库同步：已同步" });
    openPanel();
    act(() =>
      publish(
        status({
          eventSequence: 2,
          state: "syncing",
          pendingChanges: 4,
          pendingUploads: 2,
          pendingDownloads: 1,
          uploadedBytes: 1024,
          totalUploadBytes: 2048,
          downloadedBytes: 512,
          totalDownloadBytes: 4096,
          mediaReady: false,
        }),
      ),
    );
    expect(screen.getByRole("button", { name: "音乐库同步：正在上传" })).toBeVisible();
    expect(screen.getByText("上传：2 项待处理")).toBeInTheDocument();
    expect(screen.getByText("下载：1 项待处理")).toBeInTheDocument();
    expect(screen.getByLabelText("上传进度")).toHaveAttribute("value", "1024");
    expect(screen.getByLabelText("上传进度")).toHaveAttribute("max", "2048");
    expect(screen.getByLabelText("下载进度")).toHaveAttribute("value", "512");
    expect(screen.getByText("1.0 KB / 2.0 KB")).toBeInTheDocument();
  });

  it("keeps unknown queue statistics distinct from an empty queue", async () => {
    vi.mocked(lanSyncAdapter.status).mockResolvedValue(
      status({
        pendingChanges: undefined,
        pendingUploads: undefined,
        pendingDownloads: undefined,
        uploadedBytes: undefined,
        totalUploadBytes: undefined,
        downloadedBytes: undefined,
        totalDownloadBytes: undefined,
      }),
    );
    render(<SyncToolbar />);
    await screen.findByRole("button", { name: "音乐库同步：正在核对同步状态" });
    openPanel();
    expect(screen.getByText("待确认修改：待统计")).toBeInTheDocument();
    expect(screen.getByText("上传：待统计")).toBeInTheDocument();
    expect(screen.getByText("下载：待统计")).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("ignores older events and a late initial read after a newer session arrives", async () => {
    let finishRead!: (next: SyncStatus) => void;
    vi.mocked(lanSyncAdapter.status).mockReturnValue(
      new Promise((resolve) => {
        finishRead = resolve;
      }),
    );
    render(<SyncToolbar />);
    act(() => publish(status({ sessionId: 2, eventSequence: 1, libraryId: "library-b" })));
    await act(async () => finishRead(status({ eventSequence: 99, mediaReady: false })));
    act(() => publish(status({ eventSequence: 100, state: "failed" })));
    act(() => publish(status({ sessionId: 2, eventSequence: 0, mediaReady: false })));
    expect(screen.getByRole("button", { name: "音乐库同步：已同步" })).toBeVisible();
  });

  it("supports manual sync, pause, resume and retry without inventing completion", async () => {
    vi.mocked(lanSyncAdapter.syncNow).mockResolvedValueOnce(
      status({ eventSequence: 2, mediaReady: false }),
    );
    vi.mocked(lanSyncAdapter.pause).mockResolvedValue(
      status({ eventSequence: 3, state: "paused" }),
    );
    vi.mocked(lanSyncAdapter.resume).mockResolvedValue(
      status({ eventSequence: 4, state: "failed", lastError: "sync.connectionFailed" }),
    );
    render(<SyncToolbar />);
    await screen.findByRole("button", { name: "音乐库同步：已同步" });
    openPanel();
    fireEvent.click(screen.getByRole("button", { name: "立即同步" }));
    await screen.findByRole("button", { name: "音乐库同步：等待媒体就绪" });
    fireEvent.click(screen.getByRole("button", { name: "暂停同步" }));
    await screen.findByRole("button", { name: "音乐库同步：同步已暂停" });
    fireEvent.click(screen.getByRole("button", { name: "恢复同步" }));
    await screen.findByRole("button", { name: "音乐库同步：主设备离线" });
    expect(screen.getByRole("alert")).toHaveTextContent("无法连接主设备");
    vi.mocked(lanSyncAdapter.syncNow).mockResolvedValue(status({ eventSequence: 5 }));
    fireEvent.click(screen.getByRole("button", { name: "重试同步" }));
    await screen.findByRole("button", { name: "音乐库同步：已同步" });
    expect(lanSyncAdapter.syncNow).toHaveBeenCalledTimes(2);
    expect(lanSyncAdapter.pause).toHaveBeenCalledOnce();
    expect(lanSyncAdapter.resume).toHaveBeenCalledOnce();
  });

  it("allows pause during an in-flight manual cycle and discards its stale result", async () => {
    let finishSync!: (next: SyncStatus) => void;
    vi.mocked(lanSyncAdapter.syncNow).mockReturnValue(
      new Promise((resolve) => {
        finishSync = resolve;
      }),
    );
    vi.mocked(lanSyncAdapter.pause).mockResolvedValue(
      status({ eventSequence: 3, state: "paused" }),
    );
    render(<SyncToolbar />);
    await screen.findByRole("button", { name: "音乐库同步：已同步" });
    openPanel();
    fireEvent.click(screen.getByRole("button", { name: "立即同步" }));
    expect(screen.getByRole("button", { name: "暂停同步" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "暂停同步" }));
    await screen.findByRole("button", { name: "音乐库同步：同步已暂停" });
    await act(async () => finishSync(status({ eventSequence: 2 })));
    expect(screen.getByRole("button", { name: "音乐库同步：同步已暂停" })).toBeVisible();
  });

  it("closes with Escape, restores focus and releases the subscription", async () => {
    const view = render(<SyncToolbar />);
    const trigger = await screen.findByRole("button", { name: "音乐库同步：已同步" });
    trigger.focus();
    openPanel();
    expect(screen.getByRole("dialog", { name: "音乐库同步" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
    view.unmount();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});
