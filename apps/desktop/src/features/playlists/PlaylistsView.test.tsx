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

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlaylistsView } from "./PlaylistsView";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

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
}

function renderView(props: Partial<Parameters<typeof PlaylistsView>[0]> = {}) {
  return render(
    <PlaylistsView
      playlistId="pl-1"
      title="深夜"
      root=""
      existingNames={["深夜", "通勤"]}
      readOnly={false}
      {...props}
    />,
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
});
