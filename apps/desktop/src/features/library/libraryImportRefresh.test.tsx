/**
 * Import completion refreshes the view that launched it (regression).
 *
 * `reset()` used to empty the local page only. Since the query inputs did not
 * change, `useSongs` never issued another request and both 全部歌曲 and 最近添加
 * stayed stale until the user navigated away and back.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it } from "vitest";

import { LibraryWorkspace } from "./LibraryWorkspace";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke: (command: string, value: unknown) => void };
  }
).__echoTest;

const beforeImport = {
  id: "song-before",
  title: "导入前",
  artist: "艺人",
  album: "专辑",
  durationS: 180,
  favorite: false,
  playCount: 0,
  availability: "available" as const,
  relativePath: "before.flac",
};

const afterImport = {
  ...beforeImport,
  id: "song-after",
  title: "刚导入的歌曲",
  relativePath: "after.flac",
};

beforeEach(() => {
  mocks.setInvoke("song_cover_keys", {});
  mocks.setInvoke("choose_and_import_files", {
    results: [
      {
        kind: "imported",
        operationId: "operation-1",
        songId: afterImport.id,
        relativePath: afterImport.relativePath,
      },
    ],
  });
});

describe("导入完成后的资料库刷新", () => {
  it.each([
    ["all" as const, "全部歌曲", "all_songs"],
    ["recent" as const, "最近添加", "recent"],
  ])("refreshes %s after its import completes", async (view, title, command) => {
    mocks.setInvoke(command, { items: [beforeImport], isLast: true, nextCursor: null });
    if (command === "recent") {
      mocks.setInvoke(command, [beforeImport]);
    }

    render(<LibraryWorkspace view={view} title={title} root="root-1" readOnly={false} />);
    expect(await screen.findByText(beforeImport.title)).toBeInTheDocument();
    const callsBeforeImport = (
      invoke as unknown as { mock: { calls: unknown[][] } }
    ).mock.calls.filter(([called]) => called === command).length;

    mocks.setInvoke(
      command,
      command === "recent"
        ? [afterImport]
        : {
            items: [afterImport],
            isLast: true,
            nextCursor: null,
          },
    );
    fireEvent.click(screen.getByTestId("import-button"));

    expect(await screen.findByText(afterImport.title)).toBeInTheDocument();
    await waitFor(() => {
      const calls = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.filter(
        ([called]) => called === command,
      ).length;
      expect(calls).toBeGreaterThan(callsBeforeImport);
    });
  });

  it("keeps 最近添加 as a fixed chronological view", async () => {
    mocks.setInvoke("recent", [beforeImport]);

    render(<LibraryWorkspace view="recent" title="最近添加" root="root-1" readOnly={false} />);

    expect(await screen.findByText(beforeImport.title)).toBeInTheDocument();
    expect(screen.queryByTestId("sort-button")).not.toBeInTheDocument();
  });

  it("keeps a favorites sort separate from the all-songs sort", async () => {
    mocks.setInvoke("all_songs", { items: [beforeImport], isLast: true, nextCursor: null });
    mocks.setInvoke("favorites", { items: [beforeImport], isLast: true, nextCursor: null });

    const { rerender } = render(
      <LibraryWorkspace view="all" title="全部歌曲" root="root-1" readOnly={false} />,
    );
    await screen.findByText(beforeImport.title);
    fireEvent.click(screen.getByTestId("sort-button"));
    fireEvent.click(screen.getByText("歌曲名称"));

    rerender(
      <LibraryWorkspace view="favorites" title="喜欢的音乐" root="root-1" readOnly={false} />,
    );
    expect(await screen.findByTestId("sort-button")).toBeInTheDocument();

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "favorites",
        expect.objectContaining({ sort: "addedAt:desc" }),
      ),
    );
  });
});
