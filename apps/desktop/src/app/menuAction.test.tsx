/**
 * macOS 应用菜单的前端分发（issue #73）。
 *
 * 壳层把菜单里的导航/导入/搜索/设置动作以 `menu://action` 事件发给前端；
 * 播放类命令留在 Rust 侧的播放权威通道，不经这里。这些用例固定「事件 →
 * 既有前端入口」的接线：视图切换走 `selectView`、导入走品牌区同一条
 * `choose_and_import_files`、设置打开设置浮层、搜索聚焦当前视图的搜索框、
 * 目录切换/重扫描复用设置页的命令序列。未知动作必须是无操作。
 */
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it } from "vitest";

import { App } from "./App";

const mocks = (
  globalThis as unknown as {
    __echoTest: {
      setInvoke(command: string, value: unknown): void;
      emit(event: string, payload: unknown): void;
    };
  }
).__echoTest;

const SONG = {
  id: "saved-song",
  title: "已有歌曲",
  artist: "Echo",
  album: "Album",
  durationS: 120,
  favorite: false,
  playCount: 0,
  availability: "available" as const,
  relativePath: "song.flac",
};
const ARTIST = {
  kind: "artist",
  artistKey: "alice",
  artist: "Alice",
  name: "Alice",
  songCount: 1,
  coverKey: null,
  hasCustomCover: false,
};

/** The shell reports its initial view so the macOS 视图 check state syncs. */
function reportedViews(): string[] {
  return (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls
    .filter(([command]) => command === "report_library_view")
    .map(([, args]) => (args as { view: string }).view);
}

async function activateLibrary() {
  await act(async () => {
    render(<App />);
  });
  mocks.setInvoke("choose_library_root", {
    configured: true,
    readOnly: false,
    activeRoot: "root-1",
  });
  mocks.setInvoke("library_status", {
    configured: true,
    readOnly: false,
    unavailable: false,
    scanning: false,
    activeRoot: "root-1",
  });
  mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
  mocks.setInvoke("playlists", []);
  mocks.setInvoke("library_counts", {
    all: 1,
    recent: 1,
    favorites: 0,
    artists: 1,
    albums: 0,
  });
  mocks.setInvoke("catalog_collections", [ARTIST]);
  mocks.setInvoke("catalog_collection_songs", [SONG]);
  mocks.setInvoke("start_scan", {
    generation: 2,
    cancelled: false,
    progress: { state: "completed" },
  });
  fireEvent.click(screen.getByRole("button", { name: "选择音乐库目录" }));
  await screen.findByTestId("workspace");
}

beforeEach(() => {
  reportedViews().length = 0;
});

describe("menu://action dispatch", () => {
  it("switches the workspace view and re-reports it for the menu check state", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");
    expect(reportedViews()).toContain("all");

    mocks.emit("menu://action", { action: "view:artists" });
    await screen.findByRole("button", { name: "打开歌手 Alice" });
    await waitFor(() => expect(reportedViews()).toContain("artists"));
  });

  it("ignores unknown view payloads instead of routing to a bogus view", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");

    mocks.emit("menu://action", { action: "view:bogus" });
    // The current view stays 全部歌曲 and no bogus report is sent.
    expect(await screen.findByText("已有歌曲")).toBeInTheDocument();
    expect(reportedViews()).not.toContain("bogus");
  });

  it("runs the same import flow as the brand-area button", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");

    mocks.emit("menu://action", { action: "import" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("choose_and_import_files", {}));
  });

  it("opens the settings layer", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");

    mocks.emit("menu://action", { action: "settings" });
    expect(await screen.findByTestId("settings")).toBeInTheDocument();
  });

  it("focuses the workspace search box of the routed view", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");

    mocks.emit("menu://action", { action: "search" });
    await waitFor(() => {
      expect(screen.getByTestId("search-input")).toHaveFocus();
    });
  });

  it("rescans the active root with the settings-page sequence", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");

    mocks.emit("menu://action", { action: "rescan" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_scan", { root: "root-1" }));
  });

  it("treats a cancelled directory picker as a no-op, then refreshes on success", async () => {
    await activateLibrary();
    await screen.findByText("已有歌曲");
    const statusCalls = () =>
      (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.filter(
        ([command]) => command === "library_status",
      ).length;
    await waitFor(() => expect(statusCalls()).toBeGreaterThanOrEqual(1));
    const before = statusCalls();

    mocks.setInvoke("choose_library_root", null);
    mocks.emit("menu://action", { action: "choose_library_root" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("choose_library_root", {}));
    // 取消是无操作：不触发状态刷新。
    expect(statusCalls()).toBe(before);

    mocks.setInvoke("choose_library_root", {
      configured: true,
      readOnly: false,
      activeRoot: "root-2",
    });
    mocks.emit("menu://action", { action: "choose_library_root" });
    await waitFor(() => expect(statusCalls()).toBeGreaterThan(before));
  });
});
