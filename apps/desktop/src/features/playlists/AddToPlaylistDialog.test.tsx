/**
 * Task 10.9 — add-to-playlist selector: multi-playlist membership via a single
 * `add_to_playlists(song, targets)` mutation; cancel never mutates; the targets
 * are exactly the selected playlist ids.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AddToPlaylistDialog } from "./AddToPlaylistDialog";
import { ToastView } from "../../app/ToastView";

vi.mock("../../bridge", () => ({
  bridge: {
    call: vi.fn(),
  },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

describe("AddToPlaylistDialog (task 10.9)", () => {
  it("sends only the chosen playlist targets in one mutation, then closes", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([
      { id: "pl-1", name: "Chill", memberCount: 3 },
      { id: "pl-2", name: "Focus", memberCount: 5 },
    ] as never);
    call.mockResolvedValueOnce(undefined as never); // add_to_playlists

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(
      <AddToPlaylistDialog songId="song-9" songTitle="心房" onClose={onClose} onDone={onDone} />,
    );

    await screen.findByLabelText("Chill");
    // The prototype's sub line names the song being added.
    expect(screen.getByText("将「心房」添加到：")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Focus"));
    fireEvent.click(screen.getByText("确认"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("add_to_playlists", {
        song: "song-9",
        targets: ["pl-2"],
      }),
    );
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps the authoritative picker open when membership commit fails", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill", memberCount: 3 }] as never);
    call.mockRejectedValueOnce(new Error("offline"));
    const onClose = vi.fn();
    const onDone = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={onDone} />);

    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByLabelText("Chill"));
    fireEvent.click(screen.getByText("确认"));

    expect(await screen.findByText("添加失败，请重试")).toBeInTheDocument();
    expect(screen.getByTestId("add-to-playlist-dialog")).toBeInTheDocument();
    expect(screen.getByLabelText("Chill")).toHaveAttribute("aria-selected", "true");
    expect(onDone).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("runs a multi-song add sequentially and keeps partial failures visible", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill", memberCount: 3 }] as never);
    call.mockResolvedValueOnce(undefined as never);
    call.mockRejectedValueOnce(Object.assign(new Error("duplicate"), { code: "conflict" }));

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(
      <>
        <AddToPlaylistDialog songIds={["song-1", "song-2"]} onClose={onClose} onDone={onDone} />
        <ToastView />
      </>,
    );

    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByLabelText("Chill"));
    fireEvent.click(screen.getByText("确认"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("add_to_playlists", {
        song: "song-1",
        targets: ["pl-1"],
      }),
    );
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("add_to_playlists", {
        song: "song-2",
        targets: ["pl-1"],
      }),
    );
    expect(await screen.findByText("添加到歌单：成功 1，跳过 1")).toBeInTheDocument();
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("does not mutate and reports when no playlist is selected", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill", memberCount: 0 }] as never);

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={onDone} />);
    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByText("确认"));

    expect(await screen.findByText("请至少选择一个歌单")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("add_to_playlists", expect.anything());
    expect(onDone).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("disables playlist mutations on a read-only library", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([] as never);
    render(
      <AddToPlaylistDialog
        songId="song-9"
        root="root-1"
        readOnly
        onClose={vi.fn()}
        onDone={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("playlist-picker-new")).toBeDisabled();
      expect(screen.getByRole("button", { name: "确认" })).toBeDisabled();
    });
  });

  it("shows the prototype's empty-list copy when there is no playlist yet", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([] as never);

    render(<AddToPlaylistDialog songId="song-9" onClose={vi.fn()} onDone={vi.fn()} />);
    expect(await screen.findByText("还没有歌单，先创建一个吧。")).toBeInTheDocument();
    expect(screen.getByText("新建歌单")).toBeInTheDocument();
  });

  it("passes the active root into the inline create dialog", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([] as never);
    render(
      <AddToPlaylistDialog songId="song-9" root="root-1" onClose={vi.fn()} onDone={vi.fn()} />,
    );

    expect(await screen.findByText("还没有歌单，先创建一个吧。")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("playlist-picker-new"));
    const input = await screen.findByLabelText<HTMLInputElement>("新歌单名称");
    // Opening the nested dialog must leave focus in its field. If the picker
    // restores focus while being paused, macOS CJK IME composition is cancelled.
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: "通勤" } });
    fireEvent.click(screen.getByText("创建歌单"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("create_playlist", { root: "root-1", name: "通勤" }),
    );
  });

  it("keeps CJK composition in the inline create field until it is committed", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([] as never);
    render(
      <AddToPlaylistDialog songId="song-9" root="root-1" onClose={vi.fn()} onDone={vi.fn()} />,
    );

    await screen.findByText("还没有歌单，先创建一个吧。");
    fireEvent.click(screen.getByTestId("playlist-picker-new"));
    const input = await screen.findByLabelText<HTMLInputElement>("新歌单名称");

    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "tongqin" } });
    expect(input).toHaveValue("tongqin");
    expect(screen.queryByRole("alert")).toBeNull();

    fireEvent.compositionEnd(input, { data: "通勤" });
    fireEvent.change(input, { target: { value: "通勤" } });
    expect(input).toHaveValue("通勤");
    expect(input).toHaveFocus();
  });

  it("selects a playlist created from the picker so the original song can be added", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([] as never);
    call.mockResolvedValueOnce("pl-new" as never); // create_playlist
    call.mockResolvedValueOnce([{ id: "pl-new", name: "通勤", memberCount: 0 }] as never); // refresh after creation
    call.mockResolvedValueOnce(undefined as never); // add_to_playlists

    render(
      <AddToPlaylistDialog songId="song-9" root="root-1" onClose={vi.fn()} onDone={vi.fn()} />,
    );
    await screen.findByText("还没有歌单，先创建一个吧。");
    fireEvent.click(screen.getByTestId("playlist-picker-new"));
    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "通勤" } });
    fireEvent.click(screen.getByText("创建歌单"));

    const created = await screen.findByLabelText("通勤");
    await waitFor(() => expect(created).toHaveAttribute("aria-selected", "true"));
    fireEvent.click(screen.getByText("确认"));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("add_to_playlists", {
        song: "song-9",
        targets: ["pl-new"],
      }),
    );
  });

  it("cancelling closes without sending any mutation", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill", memberCount: 0 }] as never);

    const onClose = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={() => {}} />);
    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByText("取消"));

    expect(call).not.toHaveBeenCalledWith("add_to_playlists", expect.anything());
    expect(onClose).toHaveBeenCalled();
  });
});
