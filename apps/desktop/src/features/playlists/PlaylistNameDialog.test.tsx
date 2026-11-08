/**
 * Task 10.9 — the shared `.playlist-name-dialog`: 40-grapheme rule, the
 * prototype's own blank / 竖线 / 同名 rules, and the create / rename command
 * contract. A rejected name never calls the mutation.
 *
 * Cover picks go through `preview_cover`: the WebView only ever sees the
 * shell's bounded JPEG as a `data:` URL, never a blob URL or the original
 * image (DAS-R23 / DAS-R25).
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { PlaylistNameDialog } from "./PlaylistNameDialog";

const { assetUrl } = vi.hoisted(() => ({
  assetUrl: vi.fn((key: string, size = "list") => `cover://${key}?s=${size}`),
}));

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn(), assetUrl },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

/** A fixed stand-in for the shell's bounded JPEG (jsdom never decodes it). */
const PREVIEW_JPEG_BASE64 = "cHJldmlldy1qcGVn";
const PREVIEW_DATA_URL = `data:image/jpeg;base64,${PREVIEW_JPEG_BASE64}`;

const defaultCall = (command: string) =>
  Promise.resolve(command === "preview_cover" ? PREVIEW_JPEG_BASE64 : undefined);

beforeEach(() => {
  call.mockReset();
  call.mockImplementation(defaultCall as never);
  assetUrl.mockClear();
});

function renderEdit(extra: Record<string, unknown> = {}) {
  return render(
    <PlaylistNameDialog
      mode="edit"
      playlistId="pl-1"
      initialName="深夜"
      existingNames={[]}
      onClose={vi.fn()}
      onDone={vi.fn()}
      {...extra}
    />,
  );
}

function pickCover(name: string, content: string) {
  const input = document.querySelector('input[type="file"]') as HTMLInputElement;
  fireEvent.change(input, {
    target: { files: [new File([content], name, { type: "image/png" })] },
  });
}

function previewSrc() {
  return screen.getByAltText("当前歌单封面").getAttribute("src");
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
    renderCreate();

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "春".repeat(41) } });
    fireEvent.click(screen.getByText("创建歌单"));

    expect(await screen.findByText(/名称不能超过 40 个字符/)).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("create_playlist", expect.anything());
  });

  it("rejects a blank name and a duplicate name with the prototype's copy", async () => {
    renderCreate(["深夜"]);

    fireEvent.click(screen.getByText("创建歌单"));
    expect(await screen.findByText("请输入歌单名称。")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "深夜" } });
    fireEvent.click(screen.getByText("创建歌单"));
    expect(await screen.findByText("已存在同名歌单，请换一个名称。")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("create_playlist", expect.anything());
  });

  it("rejects a create when no active root is available", async () => {
    renderCreate();
    fireEvent.change(screen.getByLabelText("新歌单名称"), { target: { value: "通勤" } });
    fireEvent.click(screen.getByText("创建歌单"));
    expect(await screen.findByText("当前音乐库不可用，请恢复后重试")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalled();
  });

  it("creates the playlist with the active root and trimmed name", async () => {
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
    const error = Object.assign(new Error("permission"), { code: "permission" });
    call.mockRejectedValueOnce(error);
    renderEdit();

    fireEvent.click(screen.getByText("保存"));

    expect(await screen.findByText("当前音乐库没有写入权限，请检查目录权限。")).toBeInTheDocument();
  });

  it("keeps the committed name and retries the same create instead of creating twice", async () => {
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
    renderEdit();

    pickCover("cover.png", "cover-bytes");
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(
        "preview_cover",
        expect.objectContaining({ mime: "image/png" }),
      ),
    );
    await waitFor(() => expect(previewSrc()).toBe(PREVIEW_DATA_URL));

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
    renderEdit({ initialCoverKey: "cv1-saved" });

    const oversized = new File(["x"], "huge.png", { type: "image/png" });
    Object.defineProperty(oversized, "size", { value: 5 * 1024 * 1024 + 1 });
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [oversized] } });

    expect(await screen.findByText("封面图片不能超过 5 MB。")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("preview_cover", expect.anything());
    expect(call).not.toHaveBeenCalledWith("set_playlist_cover", expect.anything());
    // The saved cover preview is untouched by the rejected pick.
    expect(previewSrc()).toBe("cover://cv1-saved?s=detail");
  });

  it("renders the picked cover preview as a bounded data URL instead of a blob", async () => {
    // jsdom 25 has no `URL.createObjectURL` at all (the setup-level stub was
    // removed with the blob path). Define a local spy purely to prove the new
    // path never calls it, then restore the environment.
    const create = vi.fn();
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: create });
    try {
      renderEdit({ initialCoverKey: "cv1-saved" });
      expect(previewSrc()).toBe("cover://cv1-saved?s=detail");
      expect(assetUrl).toHaveBeenCalledWith("cv1-saved", "detail");

      assetUrl.mockClear();
      pickCover("cover.png", "cover");
      await waitFor(() => expect(previewSrc()).toBe(PREVIEW_DATA_URL));
      // A picked image is not an asset key: it must never go through assetUrl.
      expect(assetUrl).not.toHaveBeenCalled();
      expect(create).not.toHaveBeenCalled();
    } finally {
      Reflect.deleteProperty(URL, "createObjectURL");
    }
  });

  it("releases the previous preview when another cover is picked", async () => {
    let sequence = 0;
    call.mockImplementation((async (command: string) => {
      if (command === "preview_cover") return `preview-${++sequence}`;
      return undefined;
    }) as never);
    renderEdit();

    pickCover("first.png", "first");
    await waitFor(() => expect(previewSrc()).toBe("data:image/jpeg;base64,preview-1"));
    pickCover("second.png", "second");
    await waitFor(() => expect(previewSrc()).toBe("data:image/jpeg;base64,preview-2"));
    // Only one live preview: the first data URL is no longer referenced.
    expect(document.body.innerHTML).not.toContain("preview-1");
    expect(screen.getAllByAltText("当前歌单封面")).toHaveLength(1);
  });

  it("clears the preview when the dialog unmounts or the automatic cover is restored", async () => {
    const { unmount } = renderEdit({
      initialCoverKey: "cv1-saved",
      automaticCoverKey: "cv1-auto",
      hasCustomCover: true,
    });

    pickCover("cover.png", "cover");
    await waitFor(() => expect(previewSrc()).toBe(PREVIEW_DATA_URL));

    fireEvent.click(screen.getByText("自动封面"));
    expect(previewSrc()).toBe("cover://cv1-auto?s=detail");
    expect(document.body.innerHTML).not.toContain("data:image/jpeg");

    pickCover("again.png", "again");
    await waitFor(() => expect(previewSrc()).toBe(PREVIEW_DATA_URL));
    unmount();
    expect(document.querySelector('img[src^="data:image/jpeg;base64,"]')).toBeNull();
    // Closing never writes a cover.
    expect(call).not.toHaveBeenCalledWith("set_playlist_cover", expect.anything());
  });

  it("keeps the saved cover and name when preview generation fails", async () => {
    call.mockImplementation((async (command: string) => {
      if (command === "preview_cover") throw new Error("validation");
      return undefined;
    }) as never);
    renderEdit({ initialCoverKey: "cv1-saved" });

    pickCover("broken.png", "broken");
    expect(
      await screen.findByText("图片过大或无法读取封面图片，请换一张图片。"),
    ).toBeInTheDocument();
    expect(previewSrc()).toBe("cover://cv1-saved?s=detail");
    expect(screen.getByLabelText("歌单新名称")).toHaveValue("深夜");
    expect(screen.getByRole("button", { name: "选择歌单封面图片" })).toBeInTheDocument();

    fireEvent.click(screen.getByText("保存"));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("rename_playlist", { id: "pl-1", name: "深夜" }),
    );
    // A failed preview is not a new submit-time rejection and does not invent
    // a replacement cover.
    expect(call).not.toHaveBeenCalledWith("set_playlist_cover", expect.anything());
  });

  it("does not upscale a small picked cover", async () => {
    renderEdit();

    pickCover("small.png", "x");
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(
        "preview_cover",
        expect.objectContaining({ mime: "image/png" }),
      ),
    );
    await waitFor(() => expect(previewSrc()).toBe(PREVIEW_DATA_URL));
    // The UI never builds an original-size URL; the shell owns the bounded
    // dimensions (and its own no-upscale assertion) in cover_preview.rs.
    expect(previewSrc()).not.toMatch(/^blob:/);
    expect(previewSrc()).not.toContain("data:image/png");
  });

  it("renders a saved cover at detail size and passes data URL previews through", async () => {
    renderEdit({ initialCoverKey: "cv1-saved" });

    // The playlist preview is the one 512 px surface; a saved key must ask for
    // the detail tier rather than the list default.
    expect(previewSrc()).toBe("cover://cv1-saved?s=detail");
    expect(assetUrl).toHaveBeenCalledWith("cv1-saved", "detail");

    assetUrl.mockClear();
    pickCover("cover.png", "cover");
    await waitFor(() => expect(previewSrc()).toBe(PREVIEW_DATA_URL));
    expect(assetUrl).not.toHaveBeenCalled();

    // What reaches the bridge is the base64 string — no byte array is kept in
    // this dialog's UI state.
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
