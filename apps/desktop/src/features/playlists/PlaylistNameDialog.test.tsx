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

function renderCreate(existingNames: readonly string[] = []) {
  return render(
    <PlaylistNameDialog
      mode="create"
      existingNames={existingNames}
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

  it("creates the playlist with the trimmed name and reports it", async () => {
    call.mockReset();
    call.mockResolvedValueOnce(undefined as never);
    const onDone = vi.fn();
    const onClose = vi.fn();
    render(
      <PlaylistNameDialog mode="create" existingNames={[]} onClose={onClose} onDone={onDone} />,
    );

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "  通勤  " } });
    fireEvent.click(screen.getByText("创建歌单"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("create_playlist", { root: "", name: "通勤" }),
    );
    expect(onDone).toHaveBeenCalledWith("通勤");
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
});
