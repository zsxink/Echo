/**
 * Import completion refreshes the view (playlist-search-locate-import 5.2).
 *
 * The import entry moved from `LibraryWorkspace` up to the App shell's brand
 * area, so these tests drive the *shell-owned* control (the `useImport` hook +
 * the `.brand-action` import button) next to the workspace, instead of an
 * inline button only 全部歌曲/最近添加 render. `reset()` used to empty the
 * local page only; since the query inputs did not change, `useSongs` never
 * issued another request and both views stayed stale until navigation. The
 * invalidate → onImportCommitted → fresh re-query contract is what keeps them
 * live here.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it } from "vitest";

import { LibraryWorkspace } from "./LibraryWorkspace";
import type { LibraryViewKind } from "./types";
import { useImport } from "../import";
import { invalidateLibrary } from "./libraryInvalidation";

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

/** The shell-side import surface: the button `App` renders into `.brand-actions`
 *  plus the workspace that owns the list. `onImportCommitted` mirrors the
 *  shell's playlist reload (which in turn re-reads the current view). */
function ImportShell({ view }: { view: LibraryViewKind }) {
  const { importing, runImport, renderImportDialog } = useImport({
    onImportCommitted: () => invalidateLibrary(),
  });
  return (
    <>
      <button
        type="button"
        className="brand-action"
        id="import-button"
        aria-label={importing ? "正在导入" : "导入"}
        aria-busy={importing}
        disabled={importing}
        onClick={() => void runImport()}
        data-testid="import-button"
      >
        {importing ? "正在导入" : "导入"}
      </button>
      <LibraryWorkspace
        view={view}
        title={view === "all" ? "全部歌曲" : "最近添加"}
        root="root-1"
        readOnly={false}
      />
      {renderImportDialog()}
    </>
  );
}

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

describe("导入完成后的资料库刷新（壳层入口）", () => {
  it.each([
    ["all" as const, "all_songs"],
    ["recent" as const, "recent"],
  ])("refreshes %s after its import completes", async (view, command) => {
    mocks.setInvoke(
      command,
      command === "recent"
        ? [beforeImport]
        : { items: [beforeImport], totalCount: 274, isLast: true, nextCursor: null },
    );

    render(<ImportShell view={view} />);
    expect(await screen.findByText(beforeImport.title)).toBeInTheDocument();
    expect(await screen.findByText(command === "recent" ? "1 首" : "274 首")).toBeInTheDocument();
    const callsBeforeImport = (
      invoke as unknown as { mock: { calls: unknown[][] } }
    ).mock.calls.filter(([called]) => called === command).length;

    mocks.setInvoke(
      command,
      command === "recent"
        ? [afterImport]
        : {
            items: [afterImport],
            totalCount: 275,
            isLast: true,
            nextCursor: null,
          },
    );
    fireEvent.click(screen.getByTestId("import-button"));

    expect(await screen.findByText(afterImport.title)).toBeInTheDocument();
    if (command === "all") expect(await screen.findByText("275 首")).toBeInTheDocument();
    await waitFor(() => {
      const calls = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.filter(
        ([called]) => called === command,
      ).length;
      expect(calls).toBeGreaterThan(callsBeforeImport);
    });
  });

  it("returns the import control to idle after picker cancellation", async () => {
    mocks.setInvoke("all_songs", { items: [beforeImport], isLast: true, nextCursor: null });
    let resolvePicker: (value: null) => void = () => undefined;
    mocks.setInvoke(
      "choose_and_import_files",
      new Promise<null>((resolve) => {
        resolvePicker = resolve;
      }),
    );

    render(<ImportShell view="all" />);
    await screen.findByText(beforeImport.title);

    fireEvent.click(screen.getByTestId("import-button"));
    expect(screen.getByTestId("import-button")).toBeDisabled();
    expect(screen.getByTestId("import-button")).toHaveTextContent("正在导入");

    await act(async () => {
      resolvePicker(null);
    });

    await waitFor(() => expect(screen.getByTestId("import-button")).toBeEnabled());
    expect(screen.getByTestId("import-button")).toHaveTextContent("导入");
  });

  it("shows a failure dialog when the import command is rejected", async () => {
    mocks.setInvoke("all_songs", { items: [beforeImport], isLast: true, nextCursor: null });
    mocks.setInvoke("choose_and_import_files", new Error("library unavailable"));

    render(<ImportShell view="all" />);
    await screen.findByText(beforeImport.title);
    fireEvent.click(screen.getByTestId("import-button"));

    await waitFor(() => expect(screen.getByTestId("import-dialog")).toBeInTheDocument());
    expect(screen.getByTestId("import-dialog")).toHaveTextContent("导入失败");
  });

  it("keeps 最近添加 as a fixed chronological view", async () => {
    mocks.setInvoke("recent", [beforeImport]);

    render(<ImportShell view="recent" />);

    expect(await screen.findByText(beforeImport.title)).toBeInTheDocument();
    expect(screen.queryByTestId("sort-button")).not.toBeInTheDocument();
  });

  it("keeps a favorites sort separate from the all-songs sort", async () => {
    mocks.setInvoke("all_songs", { items: [beforeImport], isLast: true, nextCursor: null });
    mocks.setInvoke("favorites", { items: [beforeImport], isLast: true, nextCursor: null });

    const { rerender } = render(<ImportShell view="all" />);
    await screen.findByText(beforeImport.title);
    fireEvent.click(screen.getByTestId("sort-button"));
    fireEvent.click(screen.getByText("歌曲名称"));

    rerender(<ImportShell view="favorites" />);
    expect(await screen.findByTestId("sort-button")).toBeInTheDocument();

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "favorites",
        expect.objectContaining({ sort: "addedAt:desc" }),
      ),
    );
  });
});
