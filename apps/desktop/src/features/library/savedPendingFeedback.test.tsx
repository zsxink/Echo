/**
 * 已保存待更新 vs 未保存 (task 8.2 / LD-01).
 *
 * The two durability failures look identical at the transport layer — both are
 * rejected IPC calls — but they mean opposite things to the user:
 *
 *   未保存   the library directory rejected the change. The optimistic heart is
 *            a lie: roll it back, re-query, show a retryable reason.
 *   已保存   the directory committed it and only the SQLite projection lags.
 *            The heart must STAY filled, the committed truth is published, and
 *            the retry carries the original operation identity — clicking the
 *            heart again would be a second like event.
 *
 * These tests pin that split, using the structured `durabilityOutcome` the
 * backend now returns (and, for older payloads, the error code alone).
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LibraryWorkspace } from "./LibraryWorkspace";
import { PlayerBar } from "../player";
import { playerStore } from "../../player/playerStore";
import { ToastView } from "../../app/ToastView";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke: (command: string, value: unknown) => void };
  }
).__echoTest;

const SONG = {
  id: "song-1",
  title: "晴天",
  artist: "周杰伦",
  album: "叶惠美",
  durationS: 239,
  favorite: false,
  playCount: 0,
  availability: "available" as const,
  relativePath: "晴天.flac",
};

const OTHER = {
  ...SONG,
  id: "song-2",
  title: "夜曲",
  album: "十一月的萧邦",
  relativePath: "夜曲.flac",
};

function snapshotOf(song: typeof SONG) {
  playerStore.publish({
    state: "playing",
    position: 10,
    duration: song.durationS,
    volume: 1,
    muted: false,
    currentQueueEntryId: "entry-1",
    currentSongId: song.id,
    queueLen: 1,
    mode: "sequential",
    currentTitle: null,
    currentCanImport: false,
    queue: [
      {
        entryId: "entry-1",
        songId: song.id,
        title: null,
        isCurrent: true,
        failed: false,
        canImport: false,
        blocked: false,
        artist: null,
        durationS: null,
        coverKey: null,
      },
    ],
  });
}

function renderSurfaces() {
  return render(
    <>
      <LibraryWorkspace view="all" title="全部歌曲" root="root-1" readOnly={false} />
      <PlayerBar root="root-1" />
      <ToastView />
    </>,
  );
}

const rowHeart = () => screen.getByRole("button", { name: /喜欢晴天|取消喜欢晴天/ });
const barHeart = () => screen.getByRole("button", { name: /喜欢当前歌曲|取消喜欢当前歌曲/ });

/** The backend's rejected envelope for a commit whose projection lags. */
function savedPendingEnvelope(operationId = "op-1") {
  return {
    code: "library_saved_pending_projection",
    messageKey: "library.savedPendingProjection",
    retryable: false,
    operationId,
    durabilityOutcome: {
      kind: "savedPendingProjection",
      operationId,
      reason: "projectionPending",
    },
  };
}

beforeEach(() => {
  mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
  mocks.setInvoke("song_detail", {
    songId: SONG.id,
    relativePath: SONG.relativePath,
    title: SONG.title,
    artist: SONG.artist,
    album: SONG.album,
    durationS: SONG.durationS,
    format: "flac",
    playCount: 0,
    favorite: SONG.favorite,
    hasCover: false,
    availability: "available",
  });
  mocks.setInvoke("song_cover_keys", {});
  mocks.setInvoke("playlists", []);
  mocks.setInvoke("library_counts", {
    all: 1,
    favorites: 0,
    recent: 1,
    artists: 1,
    albums: 1,
  });
  snapshotOf(SONG);
});

afterEach(() => {
  act(() => playerStore.publish({ ...playerStore.getSnapshot(), state: "stopped" }));
});

describe("已保存待更新 keeps the committed like on screen", () => {
  it("does not roll the row heart back when only the projection failed", async () => {
    mocks.setInvoke("set_favorite", savedPendingEnvelope());
    renderSurfaces();

    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(rowHeart());
    });

    // The directory holds the like; the heart must show it.
    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "true"));
    expect(barHeart()).toHaveAttribute("aria-pressed", "true");
    // And the user is told it is saved, not lost.
    expect(await screen.findByTestId("toast")).toHaveTextContent(
      "已保存到音乐库，正在等待更新视图",
    );
  });

  it("offers a retry of the original operation instead of a second toggle", async () => {
    mocks.setInvoke("set_favorite", savedPendingEnvelope("op-7"));
    mocks.setInvoke("retry_library_operation", undefined);
    renderSurfaces();

    const likes = () =>
      vi.mocked(invoke).mock.calls.filter(([command]) => command === "set_favorite").length;
    const before = likes();

    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(rowHeart());
    });

    const action = await screen.findByRole("button", { name: "恢复" });
    act(() => {
      fireEvent.click(action);
    });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("retry_library_operation", {
        root: "root-1",
        operation: "op-7",
      }),
    );
    // Exactly one like was submitted by this test: the retry recovered the
    // committed operation, it did not toggle the heart again.
    expect(likes()).toBe(before + 1);
  });

  it("keeps the recovery action reachable by keyboard and never auto-dismisses it", async () => {
    mocks.setInvoke("set_favorite", savedPendingEnvelope("op-kbd"));
    mocks.setInvoke("retry_library_operation", undefined);
    renderSurfaces();

    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(rowHeart());
    });

    const action = await screen.findByRole("button", { name: "恢复" });
    // A toast that carries an action waits for the user — it must not vanish
    // mid-interaction (ToastView keeps it open while it has an action).
    act(() => action.focus());
    expect(action).toHaveFocus();

    act(() => {
      fireEvent.click(action);
    });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("retry_library_operation", {
        root: "root-1",
        operation: "op-kbd",
      }),
    );
  });

  it("recognises the state from the code alone when no structured outcome is sent", async () => {
    mocks.setInvoke("set_favorite", {
      code: "library_saved_pending_recovery",
      messageKey: "library.savedPendingRecovery",
      retryable: false,
      operationId: "op-9",
    });
    renderSurfaces();

    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(rowHeart());
    });
    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "true"));
    expect(await screen.findByTestId("toast")).toHaveTextContent("等待恢复");
  });

  it("still rolls back and re-queries when the directory rejected the like", async () => {
    mocks.setInvoke("set_favorite", {
      code: "permission",
      messageKey: "library.permission",
      retryable: true,
    });
    renderSurfaces();

    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(rowHeart());
    });

    // 未保存: the optimistic state was a lie, and the authoritative re-read
    // (the pre-commit view) is what stands.
    await waitFor(() =>
      expect(
        vi.mocked(invoke).mock.calls.filter(([command]) => command === "all_songs").length,
      ).toBeGreaterThan(1),
    );
    expect(rowHeart()).toHaveAttribute("aria-pressed", "false");
  });

  it("keeps the player bar heart filled when its own toggle is saved but pending", async () => {
    mocks.setInvoke("set_favorite", savedPendingEnvelope("op-bar"));
    renderSurfaces();

    await waitFor(() => expect(barHeart()).toHaveAttribute("aria-pressed", "false"));
    await waitFor(() => expect(rowHeart()).toHaveAttribute("aria-pressed", "false"));
    act(() => {
      fireEvent.click(barHeart());
    });

    await waitFor(() => expect(barHeart()).toHaveAttribute("aria-pressed", "true"));
    expect(rowHeart()).toHaveAttribute("aria-pressed", "true");
    // The broadcast is a patch, not a replacement: the bar only knows
    // `song_detail`'s fields, so publishing a whole row from there used to
    // blank the title it never read. Both surfaces show it; the row is the one
    // that used to lose it.
    expect(rowHeart()).toHaveAccessibleName("取消喜欢晴天");
  });

  it("reports each batch item's own outcome and retries only the saved ones", async () => {
    mocks.setInvoke("all_songs", { items: [SONG, OTHER], isLast: true, nextCursor: null });
    // song-1 commits cleanly; song-2 is saved but its view lags. The batch runs
    // sequentially, so the replies are ordered.
    let call = 0;
    mocks.setInvoke("set_favorite", async () => {
      call += 1;
      if (call === 2) throw savedPendingEnvelope("op-batch");
      return { ...SONG, favorite: true };
    });
    mocks.setInvoke("retry_library_operation", undefined);
    renderSurfaces();

    await screen.findByTestId(`song-row-${OTHER.id}`);
    const firstPageReads = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === "all_songs").length;
    act(() => {
      fireEvent.click(screen.getByTestId("selection-mode-button"));
    });
    act(() => {
      fireEvent.click(screen.getByTestId(`song-select-${SONG.id}`));
      fireEvent.click(screen.getByTestId(`song-select-${OTHER.id}`));
    });
    act(() => {
      fireEvent.contextMenu(screen.getByTestId(`song-row-${SONG.id}`));
    });
    act(() => {
      fireEvent.click(screen.getByTestId("batch-favorite"));
    });

    // Per-item feedback, never a flat "失败": the saved item is not a loss.
    const toast = await screen.findByTestId("toast");
    await waitFor(() => expect(toast).toHaveTextContent("批量操作：成功 1，已保存待更新 1"));
    expect(toast).not.toHaveTextContent("失败");
    // Both rows show the like the directory holds.
    expect(screen.getByRole("button", { name: "取消喜欢晴天" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "取消喜欢夜曲" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    // The pending item did not trigger a re-read of the lagging projection:
    // that read would have shown the saved like as lost.
    const queries = () =>
      vi.mocked(invoke).mock.calls.filter(([command]) => command === "all_songs").length;
    const retries = () =>
      vi.mocked(invoke).mock.calls.filter(([command]) => command === "retry_library_operation")
        .length;
    expect(queries()).toBe(firstPageReads);
    const retriesBefore = retries();

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "恢复" }));
    });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("retry_library_operation", {
        root: "root-1",
        operation: "op-batch",
      }),
    );
    // One retry, by the original identity — never a second like event.
    expect(retries()).toBe(retriesBefore + 1);
  });
});
