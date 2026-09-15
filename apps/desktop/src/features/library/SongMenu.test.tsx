/**
 * Task 10.8 — read-only song detail + reveal + delete confirm/undo/forward-error.
 *
 * Acceptance: the detail shows only a relative path (never absolute); reveal
 * never fabricates success; delete shows a real confirmation and a 10-second
 * undo; a cancel or a failed delete never produces a fake-deleted state.
 *
 * Task 10.6 — "下一首播放" and "加入播放队列": the menu distinguishes play-next
 * (insert after current) from enqueue (append), bound to the SongId that opened
 * the menu.
 *
 * The 撤销 affordance is the prototype's toast (`.toast` + `.toast-action`), so
 * the shell's `<ToastView />` is rendered alongside the menu — exactly as the
 * real app does with one global toast.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SongView } from "../../ipc/ipc-types.generated";
import { SongMenu } from "./SongMenu";
import { ToastView } from "../../app/ToastView";

function makeSong(overrides: Partial<SongView> = {}): SongView {
  return {
    id: "song-1",
    title: "Lacquer Love",
    artist: "Echo Unit",
    album: "Velvet",
    durationS: 210,
    favorite: false,
    playCount: 3,
    availability: "available",
    relativePath: "Artists/Echo Unit/Lacquer Love.flac",
    ...overrides,
  };
}

function renderMenu(song: SongView, overrides: Partial<Parameters<typeof SongMenu>[0]> = {}) {
  return render(
    <>
      <SongMenu
        song={song}
        root=""
        readOnly={false}
        onClose={overrides.onClose ?? vi.fn()}
        onPlay={overrides.onPlay ?? vi.fn()}
        onPlayNext={overrides.onPlayNext}
        onEnqueue={overrides.onEnqueue}
        onFavorite={overrides.onFavorite ?? vi.fn()}
        onRefresh={overrides.onRefresh ?? vi.fn()}
      />
      <ToastView />
    </>,
  );
}

describe("SongMenu task 10.8", () => {
  it("shows only a relative path in the read-only detail — never an absolute path", () => {
    const song = makeSong({ relativePath: "Artists/Echo Unit/Lacquer Love.flac" });
    renderMenu(song);
    fireEvent.click(screen.getByText("显示歌曲详情"));

    const relative = screen.getByTestId("detail-relative-path");
    expect(relative).toHaveTextContent("Artists/Echo Unit/Lacquer Love.flac");
    // No absolute path component leaks into the dialog.
    expect(screen.queryByText(/^\/Users\//)).toBeNull();
    expect(screen.queryByText(/^C:/)).toBeNull();
  });

  it("opens a real delete confirmation and never marks deleted before a confirm is chosen", () => {
    const onRefresh = vi.fn();
    renderMenu(makeSong(), { onRefresh });
    fireEvent.click(screen.getByText("删除"));
    // Confirmation is shown; the song is NOT deleted yet.
    expect(screen.getByText("删除「Lacquer Love」？")).toBeInTheDocument();
    expect(screen.getByText("歌曲会移至回收站。你仍可立即撤销本次操作。")).toBeInTheDocument();
    expect(onRefresh).not.toHaveBeenCalled();
    // The menu is replaced by the confirmation, as the prototype does.
    expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument();
    expect(screen.queryByTestId("toast")).not.toBeInTheDocument();
  });

  it("dispatches delete and shows the 10-second undo affordance on success", async () => {
    // @ts-expect-error test hook
    globalThis.__echoTest.setInvoke("delete_song", "op-123");
    const onRefresh = vi.fn();
    renderMenu(makeSong(), { onRefresh });

    fireEvent.click(screen.getByText("删除"));
    fireEvent.click(screen.getByText("移至回收站"));

    const toast = await screen.findByTestId("toast");
    expect(toast).toBeInTheDocument();
    expect(screen.getByText(/10 秒内可撤销/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "撤销" })).toBeInTheDocument();
    expect(onRefresh).toHaveBeenCalled();
  });

  it("shows an error and keeps the song when delete fails — no fake deletion", async () => {
    renderMenu(makeSong());
    // No mock → the delete command rejects (setup default) → error path.
    fireEvent.click(screen.getByText("删除"));
    fireEvent.click(screen.getByText("移至回收站"));

    expect(await screen.findByText("删除失败，请重试")).toBeInTheDocument();
    // No undo affordance appears for a failed delete.
    expect(screen.queryByTestId("toast")).not.toBeInTheDocument();
  });

  it("cancelling the confirmation leaves the song untouched", () => {
    const onRefresh = vi.fn();
    renderMenu(makeSong(), { onRefresh });
    fireEvent.click(screen.getByText("删除"));
    fireEvent.click(screen.getByText("取消"));
    expect(onRefresh).not.toHaveBeenCalled();
    expect(screen.queryByTestId("toast")).not.toBeInTheDocument();
  });
});

describe("SongMenu task 10.6", () => {
  it("next-play dispatches onPlayNext (not onPlay)", () => {
    const onPlay = vi.fn();
    const onPlayNext = vi.fn();
    const onEnqueue = vi.fn();
    renderMenu(makeSong(), { onPlay, onPlayNext, onEnqueue });

    fireEvent.click(screen.getByText("下一首播放"));
    expect(onPlayNext).toHaveBeenCalledTimes(1);
    expect(onPlay).not.toHaveBeenCalled();
  });

  it("enqueue appends to the queue via onEnqueue", () => {
    const onPlay = vi.fn();
    const onEnqueue = vi.fn();
    renderMenu(makeSong(), { onPlay, onEnqueue });

    fireEvent.click(screen.getByText("加入播放队列"));
    expect(onEnqueue).toHaveBeenCalledTimes(1);
    expect(onPlay).not.toHaveBeenCalled();
  });

  it("both queue actions are disabled when no handler is wired", () => {
    renderMenu(makeSong());
    expect(screen.getByText("下一首播放")).toBeDisabled();
    expect(screen.getByText("加入播放队列")).toBeDisabled();
  });
});
