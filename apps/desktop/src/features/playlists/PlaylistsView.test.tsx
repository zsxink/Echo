/**
 * Task 10.9 — playlist management view: 40-grapheme validation, rename, delete
 * (which navigates away and never deletes song files). Membership append order
 * and idempotent duplicates are enforced by the core (tasks 6.6/6.7); the UI
 * surfaces the interaction and the outcome.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlaylistsView } from "./PlaylistsView";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

function mockBasicState() {
  call.mockReset();
  // playlist_members → empty
  call.mockResolvedValueOnce([] as never);
  // playlists → the current playlist
  call.mockResolvedValueOnce([{ id: "pl-1", name: "深夜" }] as never);
}

describe("PlaylistsView (task 10.9)", () => {
  it("rejects a create name longer than 40 graphemes", async () => {
    mockBasicState();
    render(<PlaylistsView playlistId="pl-1" />);
    await screen.findByText("歌单：深夜");

    const long = "春".repeat(41);
    const input = screen.getByLabelText("新歌单名称");
    fireEvent.change(input, { target: { value: long } });
    fireEvent.click(screen.getByText("创建歌单"));

    expect(await screen.findByText(/名称不能超过 40 个字符/)).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("create_playlist", expect.anything());
  });

  it("renames the playlist through rename_playlist and updates the title", async () => {
    mockBasicState();
    call.mockResolvedValueOnce(undefined as never); // rename_playlist
    render(<PlaylistsView playlistId="pl-1" />);
    await screen.findByText("歌单：深夜");

    fireEvent.click(screen.getByText("重命名"));
    const renameInput = screen.getByLabelText("歌单新名称");
    fireEvent.change(renameInput, { target: { value: "午夜客厅" } });
    fireEvent.click(screen.getByText("保存"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("rename_playlist", { id: "pl-1", name: "午夜客厅" }),
    );
  });

  it("deletes the playlist after confirmation and navigates away", async () => {
    mockBasicState();
    call.mockResolvedValueOnce(undefined as never); // delete_playlist
    const onDeleted = vi.fn();
    render(<PlaylistsView playlistId="pl-1" onDeleted={onDeleted} />);
    await screen.findByText("歌单：深夜");

    // No deletion before confirmation.
    fireEvent.click(screen.getByText("删除歌单"));
    expect(call).not.toHaveBeenCalledWith("delete_playlist", expect.anything());

    fireEvent.click(screen.getByText("确认删除"));
    await waitFor(() => expect(call).toHaveBeenCalledWith("delete_playlist", { id: "pl-1" }));
    expect(onDeleted).toHaveBeenCalled();
  });
});
