import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { ToastView } from "../../app/ToastView";
import { LibraryWorkspace } from "./LibraryWorkspace";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke: (command: string, value: unknown) => void };
  }
).__echoTest;

const SONGS = [
  {
    id: "song-1",
    title: "晴天",
    artist: "周杰伦",
    album: "叶惠美",
    durationS: 239,
    favorite: false,
    playCount: 0,
    availability: "available" as const,
    relativePath: "晴天.flac",
  },
  {
    id: "song-2",
    title: "夜曲",
    artist: "周杰伦",
    album: "十一月的萧邦",
    durationS: 225,
    favorite: false,
    playCount: 0,
    availability: "missing" as const,
    relativePath: "夜曲.flac",
  },
];

function renderWorkspace() {
  return render(
    <>
      <LibraryWorkspace view="all" title="全部歌曲" root="root-1" readOnly={false} />
      <ToastView />
    </>,
  );
}

beforeEach(() => {
  mocks.setInvoke("all_songs", { items: SONGS, isLast: true, nextCursor: null });
  mocks.setInvoke("song_cover_keys", {});
  mocks.setInvoke("library_counts", {
    all: 2,
    favorites: 0,
    recent: 2,
    artists: 1,
    albums: 2,
  });
  mocks.setInvoke("playlists", []);
});

describe("LibraryWorkspace batch operations", () => {
  async function enterSelectionMode() {
    fireEvent.click(screen.getByTestId("selection-mode-button"));
  }

  it("makes an unselected right-click row the sole selection and preserves a selected set", async () => {
    renderWorkspace();
    await screen.findByTestId("song-row-song-1");

    expect(screen.queryByTestId("song-select-song-1")).toBeNull();
    await enterSelectionMode();
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    expect(screen.getByTestId("batch-song-menu")).toHaveTextContent("已选 1 首歌曲");

    fireEvent.click(screen.getByTestId("song-select-song-2"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    expect(screen.getByTestId("batch-song-menu")).toHaveTextContent("已选 2 首歌曲");
  });

  it("clears selection when the search scope changes", async () => {
    mocks.setInvoke("search", { items: [], isLast: true, nextCursor: null });
    renderWorkspace();
    await screen.findByTestId("song-row-song-1");

    await enterSelectionMode();
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    expect(screen.getByTestId("song-select-song-1")).toHaveAttribute("aria-pressed", "true");

    fireEvent.change(screen.getByTestId("search-input"), { target: { value: "不存在" } });
    await waitFor(() => expect(screen.queryByTestId("song-select-song-1")).toBeNull(), {
      timeout: 500,
    });
  });

  it("reports unavailable items while reconciling a mixed favorite batch", async () => {
    mocks.setInvoke("set_favorite", { ...SONGS[0], favorite: true });
    renderWorkspace();
    await screen.findByTestId("song-row-song-1");

    await enterSelectionMode();
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    fireEvent.click(screen.getByTestId("song-select-song-2"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    fireEvent.click(screen.getByTestId("batch-favorite"));

    await waitFor(() =>
      expect(screen.getByTestId("toast")).toHaveTextContent("批量操作：成功 1，跳过 1"),
    );
  });

  it("offers one undo action for successful deletes in a partial batch", async () => {
    mocks.setInvoke("delete_song", "operation-1");
    mocks.setInvoke("undo_delete", undefined);
    renderWorkspace();
    await screen.findByTestId("song-row-song-1");

    await enterSelectionMode();
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    fireEvent.click(screen.getByTestId("song-select-song-2"));
    fireEvent.contextMenu(screen.getByTestId("song-row-song-1"));
    fireEvent.click(screen.getByTestId("batch-delete"));
    fireEvent.click(screen.getByText("批量移至回收站"));

    await waitFor(() =>
      expect(screen.getByTestId("toast")).toHaveTextContent("批量删除：成功 1，跳过 1"),
    );
    fireEvent.click(screen.getByRole("button", { name: "撤销" }));

    await waitFor(() =>
      expect(screen.getByTestId("toast")).toHaveTextContent("撤回删除：成功 1，失败 0"),
    );
  });
});
