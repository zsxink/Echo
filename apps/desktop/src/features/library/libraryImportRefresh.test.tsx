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
    fireEvent.click(await screen.findByText("选择文件并导入"));

    expect(await screen.findByText(afterImport.title)).toBeInTheDocument();
    await waitFor(() => {
      const calls = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.filter(
        ([called]) => called === command,
      ).length;
      expect(calls).toBeGreaterThan(callsBeforeImport);
    });
  });
});
