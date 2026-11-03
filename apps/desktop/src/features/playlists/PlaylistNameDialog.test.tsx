/**
 * Task 10.9 — the shared `.playlist-name-dialog`: 40-grapheme rule, the
 * prototype's own blank / 竖线 / 同名 rules, and the create / rename command
 * contract. A rejected name never calls the mutation.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PlaylistNameDialog } from "./PlaylistNameDialog";

const { assetUrl } = vi.hoisted(() => ({
  assetUrl: vi.fn((key: string, size = "list") => `cover://${key}?s=${size}`),
}));

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn(), assetUrl },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

function pickCover(name: string, content: string) {
  const input = document.querySelector('input[type="file"]') as HTMLInputElement;
  fireEvent.change(input, {
    target: { files: [new File([content], name, { type: "image/png" })] },
  });
}

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
    expect(await screen.findByText("当前音乐库不可用，请恢复后重试")).toBeInTheDocument();
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

    expect(await screen.findByText("当前音乐库没有写入权限，请检查目录权限。")).toBeInTheDocument();
  });

  it("keeps the committed name and retries the same create instead of creating twice", async () => {
    call.mockReset();
    // 已保存待更新: the directory committed the playlist, the view lags.
    call.mockRejectedValueOnce(
      Object.assign(new Error("saved"), {
        code: "library_saved_pending_projection",
        messageKey: "library.savedPendingProjection",
        retryable: false,
        operationId: "op-create",
        durabilityOutcome: {
          kind: "savedPendingProjection",
          operationId: "op-create",
          reason: "projectionPending",
        },
      }),
    );
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

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "通勤" } });
    fireEvent.click(screen.getByText("创建歌单"));

    expect(await screen.findByText(/歌单已保存到音乐库，等待更新视图/)).toBeInTheDocument();
    // 保存与显示更新分离: the typed name is NOT cleared and the dialog stays
    // open — clearing it would push the user into a second playlist.
    expect(screen.getByLabelText("新歌单名称")).toHaveValue("通勤");
    expect(onClose).not.toHaveBeenCalled();

    call.mockResolvedValueOnce(undefined as never);
    fireEvent.click(screen.getByRole("button", { name: "恢复已保存资料" }));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("retry_library_operation", {
        root: "root-1",
        operation: "op-create",
      }),
    );
    // One create, one recovery of that same operation — never a second create.
    expect(call.mock.calls.filter(([command]) => command === "create_playlist")).toHaveLength(1);
    expect(onDone).toHaveBeenCalledWith("通勤");
    expect(onClose).toHaveBeenCalled();
  });

  it("says the saved rename is still in the library when its retry fails", async () => {
    call.mockReset();
    call.mockRejectedValueOnce(
      Object.assign(new Error("saved"), {
        code: "library_saved_pending_recovery",
        messageKey: "library.savedPendingRecovery",
        retryable: false,
        operationId: "op-rename",
      }),
    );
    const onClose = vi.fn();
    render(
      <PlaylistNameDialog
        mode="edit"
        playlistId="pl-1"
        root="root-1"
        initialName="深夜"
        existingNames={[]}
        onClose={onClose}
        onDone={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("歌单新名称"), { target: { value: "凌晨" } });
    fireEvent.click(screen.getByText("保存"));
    expect(await screen.findByText(/新名称已保存到音乐库，等待更新视图/)).toBeInTheDocument();

    call.mockRejectedValueOnce(new Error("volume unavailable"));
    fireEvent.click(screen.getByRole("button", { name: "恢复已保存资料" }));

    // Never "未保存" and never an invitation to rename again: the change is
    // still in the library, only its view has not caught up.
    expect(
      await screen.findByText("已保存资料仍在音乐库中，更新尚未完成。请检查目录后重试。"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("歌单新名称")).toHaveValue("凌晨");
    expect(onClose).not.toHaveBeenCalled();
    expect(call.mock.calls.filter(([command]) => command === "rename_playlist")).toHaveLength(1);
  });

  it("sends the picked cover as a base64 payload instead of a byte array", async () => {
    call.mockReset();
    call.mockResolvedValue(undefined as never);
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

    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, {
      target: { files: [new File(["cover-bytes"], "cover.png", { type: "image/png" })] },
    });
    await waitFor(() =>
      expect(screen.getByAltText("当前歌单封面").getAttribute("src")).toMatch(/^blob:/),
    );

    fireEvent.click(screen.getByText("保存"));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(
        "set_playlist_cover",
        expect.objectContaining({ id: "pl-1", mime: "image/png" }),
      ),
    );
    const payload = call.mock.calls.find(([command]) => command === "set_playlist_cover")?.[1] as {
      bytes: unknown;
    };
    expect(payload.bytes).toBe("Y292ZXItYnl0ZXM=");
    expect(Array.isArray(payload.bytes)).toBe(false);
  });

  it("rejects a picked cover larger than the size limit", async () => {
    call.mockReset();
    render(
      <PlaylistNameDialog
        mode="edit"
        playlistId="pl-1"
        initialName="深夜"
        initialCoverKey="cv1-saved"
        existingNames={[]}
        onClose={vi.fn()}
        onDone={vi.fn()}
      />,
    );

    const oversized = new File(["x"], "huge.png", { type: "image/png" });
    Object.defineProperty(oversized, "size", { value: 5 * 1024 * 1024 + 1 });
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [oversized] } });

    expect(await screen.findByText("封面图片不能超过 5 MB。")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("set_playlist_cover", expect.anything());
    // The saved cover preview is untouched by the rejected pick.
    expect(screen.getByAltText("当前歌单封面")).toHaveAttribute(
      "src",
      "cover://cv1-saved?s=detail",
    );
  });

  it("releases the previous preview blob when another cover is picked", async () => {
    const create = vi.mocked(URL.createObjectURL);
    const revoke = vi.mocked(URL.revokeObjectURL);
    create.mockClear();
    revoke.mockClear();
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

    pickCover("first.png", "first");
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const first = create.mock.results[0]?.value as string;

    pickCover("second.png", "second");
    await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
    const second = create.mock.results[1]?.value as string;

    expect(revoke).toHaveBeenCalledWith(first);
    expect(revoke).not.toHaveBeenCalledWith(second);
  });

  it("releases the preview blob when the dialog unmounts", async () => {
    const create = vi.mocked(URL.createObjectURL);
    const revoke = vi.mocked(URL.revokeObjectURL);
    create.mockClear();
    revoke.mockClear();
    const { unmount } = render(
      <PlaylistNameDialog
        mode="edit"
        playlistId="pl-1"
        initialName="深夜"
        existingNames={[]}
        onClose={vi.fn()}
        onDone={vi.fn()}
      />,
    );

    pickCover("cover.png", "cover");
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const url = create.mock.results[0]?.value as string;

    unmount();
    expect(revoke).toHaveBeenCalledWith(url);
  });

  it("renders a saved cover at detail size and passes blob previews through", async () => {
    assetUrl.mockClear();
    render(
      <PlaylistNameDialog
        mode="edit"
        playlistId="pl-1"
        initialName="深夜"
        initialCoverKey="cv1-saved"
        existingNames={[]}
        onClose={vi.fn()}
        onDone={vi.fn()}
      />,
    );

    // The playlist preview is the one 512 px surface; a saved key must ask for
    // the detail tier rather than the list default.
    expect(screen.getByAltText("当前歌单封面")).toHaveAttribute(
      "src",
      "cover://cv1-saved?s=detail",
    );
    expect(assetUrl).toHaveBeenCalledWith("cv1-saved", "detail");

    // A just-picked image is a local blob URL, not a cover-asset key: it must
    // bypass `assetUrl` entirely (no `?s=` appended to an object URL).
    assetUrl.mockClear();
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, {
      target: { files: [new File(["cover"], "cover.png", { type: "image/png" })] },
    });
    await waitFor(() =>
      expect(screen.getByAltText("当前歌单封面").getAttribute("src")).toMatch(/^blob:/),
    );
    expect(assetUrl).not.toHaveBeenCalled();

    // What reaches the bridge is the base64 string — no byte array is kept in
    // this dialog's UI state (DAS-R23-S03's second half).
    call.mockClear();
    fireEvent.click(screen.getByText("保存"));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(
        "set_playlist_cover",
        expect.objectContaining({ bytes: "Y292ZXI=", mime: "image/png" }),
      ),
    );
    const submitted = call.mock.calls.find(
      ([command]) => command === "set_playlist_cover",
    )?.[1] as {
      bytes: unknown;
    };
    expect(typeof submitted.bytes).toBe("string");
  });
});
