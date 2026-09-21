/**
 * Task 10.9 — the shared `.playlist-name-dialog`: 40-grapheme rule, the
 * prototype's own blank / 竖线 / 同名 rules, and the create / rename command
 * contract. A rejected name never calls the mutation.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlaylistNameDialog } from "./PlaylistNameDialog";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

function renderCreate(existingNames: readonly string[] = [], root?: string) {
  return render(
    <PlaylistNameDialog
      mode="create"
      existingNames={existingNames}
      root={root}
      onClose={vi.fn()}
      onDone={vi.fn()}
    />,
  );
}

describe("PlaylistNameDialog (task 10.9)", () => {
  it("rejects a name longer than 40 graphemes without calling create_playlist", async () => {
    call.mockReset();
    renderCreate();

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "春".repeat(41) } });
    fireEvent.click(screen.getByText("创建歌单"));

    expect(await screen.findByText(/名称不能超过 40 个字符/)).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("create_playlist", expect.anything());
  });

  it("rejects a blank name and a duplicate name with the prototype's copy", async () => {
    call.mockReset();
    renderCreate(["深夜"]);

    fireEvent.click(screen.getByText("创建歌单"));
    expect(await screen.findByText("请输入歌单名称。")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "深夜" } });
    fireEvent.click(screen.getByText("创建歌单"));
    expect(await screen.findByText("已存在同名歌单，请换一个名称。")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("create_playlist", expect.anything());
  });

  it("rejects a create when no active root is available", async () => {
    call.mockReset();
    renderCreate();
    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "通勤" } });
    fireEvent.click(screen.getByText("创建歌单"));
    expect(await screen.findByText("当前资料库不可用，请恢复后重试")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalled();
  });

  it("creates the playlist with the active root and trimmed name", async () => {
    call.mockReset();
    call.mockResolvedValueOnce("playlist-new" as never);
    const onDone = vi.fn();
    const onClose = vi.fn();
    render(
      <PlaylistNameDialog
        mode="create"
        root="root-1"
        existingNames={[]}
        onClose={onClose}
        onDone={onDone}
      />,
    );

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "  通勤  " } });
    fireEvent.click(screen.getByText("创建歌单"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("create_playlist", { root: "root-1", name: "通勤" }),
    );
    expect(onDone).toHaveBeenCalledWith("通勤", "playlist-new");
    expect(onClose).toHaveBeenCalled();
  });

  it("edit mode renames through rename_playlist and may keep its own name", async () => {
    call.mockReset();
    call.mockResolvedValueOnce(undefined as never);
    const onDone = vi.fn();
    render(
      <PlaylistNameDialog
        mode="edit"
        playlistId="pl-1"
        initialName="深夜"
        existingNames={["深夜", "通勤"]}
        onClose={vi.fn()}
        onDone={onDone}
      />,
    );

    // Its own name is not a duplicate of itself.
    fireEvent.click(screen.getByText("保存"));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("rename_playlist", { id: "pl-1", name: "深夜" }),
    );
    expect(onDone).toHaveBeenCalledWith("深夜");
  });

  it("surfaces a read-only backend failure instead of the generic save error", async () => {
    call.mockReset();
    const error = Object.assign(new Error("permission"), { code: "permission" });
    call.mockRejectedValueOnce(error);
    render(
      <PlaylistNameDialog
        mode="edit"
        playlistId="pl-1"
        initialName="深夜"
        existingNames={[]}
        onClose={vi.fn()}
        onDone={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("保存"));

    expect(await screen.findByText("当前资料库没有写入权限，请检查目录权限。")).toBeInTheDocument();
  });
});
