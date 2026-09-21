/**
 * Task 10.9 — playlist management view: it renders the same workspace DOM the
 * prototype uses for every view (`.library-view` → `.library-head` →
 * `.table-wrap`), carries the playlist's name and member count, renames through
 * `rename_playlist`, and deletes through a real confirmation that navigates away
 * and never deletes song files.
 *
 * The 40-grapheme naming rule lives in the shared `.playlist-name-dialog` and is
 * covered by `PlaylistNameDialog.test.tsx`; membership append order and idempotent
 * duplicates are enforced by the core (tasks 6.6/6.7).
 */

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlaylistsView } from "./PlaylistsView";
import { ToastView } from "../../app/ToastView";

// `assetUrl` and `fireAndForget` are part of the surface the row menu and the
// view import from the bridge; stub the whole module so neither is `undefined`
// once a test actually opens the menu.
vi.mock("../../bridge", () => ({
  assetUrl: (key: string) => `cover://${key}`,
  bridge: { call: vi.fn(), fireAndForget: vi.fn() },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);
const fireAndForget = vi.mocked(bridge.fireAndForget);

/**
 * Command-aware bridge mock. A one-shot `mockResolvedValueOnce` is wrong here:
 * the view issues `playlist_members` first, so the queued value would land on
 * that call instead of the one under test. Dispatching on the command name keeps
 * every call deterministic regardless of order.
 */
function mockBridge(overrides: Record<string, unknown> = {}) {
  call.mockReset();
  call.mockImplementation(((command: string) =>
    Promise.resolve(command in overrides ? overrides[command] : [])) as never);
  // Delegated to the same spy, so "which commands did the view send" never
  // depends on which bridge entry point it used (fire-and-forget commands go
  // through `fireAndForget` by convention).
  fireAndForget.mockReset();
  fireAndForget.mockImplementation(((command: string, args: unknown) => {
    // Same spy, loosened to the mock's call shape: `bridge.call` is typed per
    // command, and a test double dispatches on the name.
    void (call as unknown as (c: string, a: unknown) => Promise<unknown>)(command, args);
  }) as never);
}

/** `mockBridge`, but the listed commands reject — the failure-path cases. */
function mockBridgeWhereFails(failing: readonly string[], overrides: Record<string, unknown> = {}) {
  mockBridge(overrides);
  call.mockImplementation(((command: string) => {
    if (failing.includes(command)) return Promise.reject(new Error(`${command} failed`));
    return Promise.resolve(command in overrides ? overrides[command] : []);
  }) as never);
}

/**
 * Render the view. `withToast` also mounts the shell's single toast, which the
 * feedback scenarios assert on (it is where 失败信息 actually appears).
 */
function renderView(
  props: Partial<Parameters<typeof PlaylistsView>[0]> = {},
  { withToast = false }: { withToast?: boolean } = {},
) {
  const view = (
    <PlaylistsView
      playlistId="pl-1"
      title="深夜"
      root=""
      existingNames={["深夜", "通勤"]}
      readOnly={false}
      {...props}
    />
  );
  return render(
    withToast ? (
      <>
        {view}
        <ToastView />
      </>
    ) : (
      view
    ),
  );
}

describe("PlaylistsView (task 10.9)", () => {
  it("renders the playlist inside the prototype's library-view DOM", async () => {
    mockBridge();
    const { container } = renderView();
    await screen.findByTestId("playlist-view");

    expect(container.querySelector(".workspace")).toBeNull();
    expect(container.querySelector(".library-head")).toBeInTheDocument();
    expect(container.querySelector(".table-wrap")).toBeInTheDocument();
    expect(container.querySelector(".track-table")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "深夜" })).toBeInTheDocument();
  });

  it("shows recently added playlist members first", async () => {
    mockBridge({
      playlist_members: [
        { id: "newer", title: "刚加入", favorite: false, playCount: 0, availability: "available" },
        {
          id: "older",
          title: "较早加入",
          favorite: false,
          playCount: 0,
          availability: "available",
        },
      ],
    });
    renderView();

    await screen.findByTestId("song-row-newer");
    expect(screen.getAllByTestId(/song-row-/).map((row) => row.dataset.songId)).toEqual([
      "newer",
      "older",
    ]);
  });

  it("sorts this playlist's members without changing their membership chronology default", async () => {
    mockBridge({
      playlist_members: [
        { id: "newer", title: "Zebra", favorite: false, playCount: 0, availability: "available" },
        { id: "older", title: "Apple", favorite: false, playCount: 0, availability: "available" },
      ],
    });
    renderView();
    await screen.findByTestId("song-row-newer");

    fireEvent.click(screen.getByTestId("sort-button"));
    fireEvent.click(screen.getByText("歌曲名称"));

    expect(screen.getAllByTestId(/song-row-/).map((row) => row.dataset.songId)).toEqual([
      "newer",
      "older",
    ]);
    fireEvent.click(screen.getByTestId("sort-button"));
    fireEvent.click(screen.getByText("升序"));
    expect(screen.getAllByTestId(/song-row-/).map((row) => row.dataset.songId)).toEqual([
      "older",
      "newer",
    ]);
  });

  it("renames the playlist through rename_playlist and updates the title", async () => {
    mockBridge();
    renderView();
    await screen.findByTestId("playlist-view");

    fireEvent.click(screen.getByText("编辑歌单"));
    const renameInput = screen.getByLabelText("歌单新名称");
    fireEvent.change(renameInput, { target: { value: "午夜客厅" } });
    fireEvent.click(screen.getByText("保存"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("rename_playlist", { id: "pl-1", name: "午夜客厅" }),
    );
    // The optimistic name is shown without waiting for the shell to re-read.
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "午夜客厅" })).toBeInTheDocument(),
    );
  });

  it("rejects an authoritative sibling name before issuing a rename", async () => {
    mockBridge();
    renderView();
    await screen.findByTestId("playlist-view");

    fireEvent.click(screen.getByText("编辑歌单"));
    fireEvent.change(screen.getByLabelText("歌单新名称"), { target: { value: "通勤" } });
    fireEvent.click(screen.getByText("保存"));

    expect(await screen.findByText("已存在同名歌单，请换一个名称。")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("rename_playlist", expect.anything());
  });

  it("deletes the playlist after confirmation and navigates away", async () => {
    mockBridge({ delete_playlist: undefined });
    const onDeleted = vi.fn();
    const onLibraryChanged = vi.fn();
    renderView({ onDeleted, onLibraryChanged });
    await screen.findByTestId("playlist-view");

    // No deletion before confirmation.
    fireEvent.click(screen.getByText("删除歌单"));
    expect(call).not.toHaveBeenCalledWith("delete_playlist", expect.anything());
    expect(screen.getByText("删除歌单「深夜」？")).toBeInTheDocument();

    fireEvent.click(screen.getByText("确认删除歌单"));
    await waitFor(() => expect(call).toHaveBeenCalledWith("delete_playlist", { id: "pl-1" }));
    expect(onDeleted).toHaveBeenCalled();
    expect(onLibraryChanged).toHaveBeenCalled();
  });

  it("uses the selected set for right-click batch menus", async () => {
    const members = [
      { id: "song-1", title: "晴天", favorite: false, playCount: 0, availability: "available" },
      { id: "song-2", title: "夜曲", favorite: true, playCount: 0, availability: "available" },
    ];
    mockBridge({ playlist_members: members });
    renderView();
    await screen.findByTestId("song-row-song-1");

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    expect(screen.getByTestId("batch-song-menu")).toHaveTextContent("已选 1 首歌曲");

    fireEvent.click(screen.getByTestId("song-select-song-2"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    expect(screen.getByTestId("batch-song-menu")).toHaveTextContent("已选 2 首歌曲");
  });

  it("exits multi-select when switching to another playlist", async () => {
    const members = [
      { id: "song-1", title: "晴天", favorite: false, playCount: 0, availability: "available" },
    ];
    mockBridge({ playlist_members: members });
    const { rerender } = renderView();
    await screen.findByTestId("song-row-song-1");

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    expect(screen.getByTestId("selection-mode-button")).toHaveAttribute("aria-pressed", "true");

    rerender(
      <PlaylistsView
        playlistId="pl-2"
        title="通勤"
        root=""
        existingNames={["深夜", "通勤"]}
        readOnly={false}
      />,
    );

    await waitFor(() =>
      expect(screen.getByTestId("selection-mode-button")).toHaveAttribute("aria-pressed", "false"),
    );
    expect(screen.queryByTestId("song-select-song-1")).not.toBeInTheDocument();
  });

  it("removes the selected members one by one and refreshes the playlist", async () => {
    const members = [
      { id: "song-1", title: "晴天", favorite: false, playCount: 0, availability: "available" },
      { id: "song-2", title: "夜曲", favorite: false, playCount: 0, availability: "available" },
    ];
    const onLibraryChanged = vi.fn();
    mockBridge({ playlist_members: members, remove_playlist_song: undefined });
    renderView({ onLibraryChanged });
    await screen.findByTestId("song-row-song-1");

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    fireEvent.click(screen.getByTestId("song-select-song-2"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    fireEvent.click(screen.getByTestId("batch-remove-playlist"));

    await waitFor(() => {
      expect(call).toHaveBeenCalledWith("remove_playlist_song", {
        playlist: "pl-1",
        song: "song-1",
      });
      expect(call).toHaveBeenCalledWith("remove_playlist_song", {
        playlist: "pl-1",
        song: "song-2",
      });
    });
    expect(onLibraryChanged).toHaveBeenCalledTimes(1);
  });

  it("disables batch writes for a read-only playlist", async () => {
    mockBridge({ playlist_members: [{ id: "song-1", title: "晴天", favorite: false }] });
    renderView({ readOnly: true });
    await screen.findByTestId("song-row-song-1");

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    expect(screen.getByTestId("batch-add-playlist")).toBeDisabled();
    expect(screen.queryByTestId("batch-delete")).toBeNull();
    expect(screen.getByTestId("batch-play-next")).toBeEnabled();
  });
});

/**
 * PM-R06 歌单异步操作反馈 — the failure halves.
 *
 * 移除成员 and 入队 may only report success once the backend committed, and a
 * rejected request must leave the list, the counts and the queue exactly as the
 * server confirmed them while saying what failed. Both halves are exercised
 * here on purpose: a scenario registered against a happy-path test alone would
 * be a green that never proved the failure copy or the untouched list.
 */
describe("PlaylistsView — 歌单异步操作反馈失败路径 (PM-R06)", () => {
  const MEMBERS = [
    { id: "song-1", title: "晴天", favorite: false, playCount: 0, availability: "available" },
    { id: "song-2", title: "夜曲", favorite: false, playCount: 0, availability: "available" },
  ];

  /** Open the row menu from the row's own `.song-more` control. */
  async function openRowMenu(songId: string) {
    const row = await screen.findByTestId(`song-row-${songId}`);
    fireEvent.click(within(row).getByLabelText("歌曲操作"));
    await screen.findByTestId("song-menu");
  }

  it("keeps the member in place and reports it when 从歌单移除 fails", async () => {
    const onLibraryChanged = vi.fn();
    mockBridgeWhereFails(["remove_playlist_song"], { playlist_members: MEMBERS });
    renderView({ onLibraryChanged }, { withToast: true });
    await screen.findByTestId("song-row-song-1");

    await openRowMenu("song-1");
    fireEvent.click(screen.getByText("从歌单移除"));

    // 移除失败信息
    await waitFor(() =>
      expect(screen.getByTestId("toast")).toHaveTextContent("移除歌曲失败，请重试"),
    );
    // 该成员仍显示在原位置 — every member is still there, in order.
    expect(screen.getAllByTestId(/song-row-/).map((row) => row.dataset.songId)).toEqual([
      "song-1",
      "song-2",
    ]);
    expect(screen.getByText("2 首")).toBeInTheDocument();
    // The member list is not re-read, so nothing the user was looking at is lost.
    expect(call.mock.calls.filter(([command]) => command === "playlist_members")).toHaveLength(1);
    // 导航计数不变
    expect(onLibraryChanged).not.toHaveBeenCalled();
  });

  it("never claims 加入播放队列 succeeded when the enqueue fails", async () => {
    mockBridgeWhereFails(["queue_command"], { playlist_members: MEMBERS });
    renderView({}, { withToast: true });
    await screen.findByTestId("song-row-song-1");

    await openRowMenu("song-1");
    fireEvent.click(screen.getByText("加入播放队列"));

    // 显示失败信息 …
    await waitFor(() =>
      expect(screen.getByTestId("toast")).toHaveTextContent("加入播放队列失败，请重试"),
    );
    // … and 界面不显示"已加入播放队列"的成功反馈.
    expect(screen.getByTestId("toast")).not.toHaveTextContent(/已将/);
    // The request really was attempted: this is the failure path, not a no-op.
    expect(call).toHaveBeenCalledWith("queue_command", { command: "enqueue", songId: "song-1" });
  });
});
