/**
 * fix-queue-trigger-toggle — 触发控件的手势在真实宿主里的行为。
 *
 * 之前的所有菜单测试都把宿主替换成了 stub：一个不带 onClick 的按钮，或一个
 * 只会"开"的处理器。于是"再次点击关闭"这条断言在结构上根本无法失败，而真
 * 实宿主恰恰缺的就是那个 toggle。issue #33 因此发布。
 *
 * 这里刻意渲染真实的 `App`（`LibraryWorkspace` 是它的工作区），跑真实的
 * `SongRow` → 宿主 toggle → 菜单这条链。`triggerRef` 抑制了 pointerdown 收起，
 * 所以关掉单曲菜单的必须是 click 里的 toggle —— 少一环这个测试就红。
 *
 * 批量菜单是例外，且是有意的：它按当前选中集重新打开而不是关闭（多选 spec
 * 的既有契约），`triggerRef` 在这里只保证该次按压不会先收起再重开。
 */

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { App } from "../../app/App";

const CONFIGURED = {
  configured: true,
  readOnly: false,
  unavailable: false,
  scanning: false,
  activeRoot: "/Music",
};

function makeSong(id: string, title: string) {
  return {
    id,
    title,
    artist: "Echo Unit",
    album: "Velvet",
    durationS: 210,
    favorite: false,
    playCount: 3,
    availability: "available",
    relativePath: `Artists/Echo Unit/${title}.flac`,
  };
}

async function renderWorkspace(
  songs = [makeSong("song-1", "Lacquer Love"), makeSong("song-2", "Velvet Night")],
) {
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("library_status", CONFIGURED);
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("all_songs", {
    items: songs,
    isLast: true,
    nextCursor: null,
  });
  // @ts-expect-error test hook
  globalThis.__echoTest.setInvoke("set_volume", { ok: true });
  render(<App />);
  await screen.findByTestId("song-row-song-1");
}

/** The real gesture the `.song-more` control makes: pointerdown then click. */
function press(element: HTMLElement) {
  fireEvent.pointerDown(element);
  fireEvent.click(element);
}

/** The real gesture the row makes for the batch menu: a secondary-button press. */
function rightPress(row: HTMLElement) {
  fireEvent.pointerDown(row, { button: 2, buttons: 2 });
  fireEvent.contextMenu(row);
}

/**
 * 121 首歌，`song-40` 落在初始渲染窗口内（jsdom 的 `clientHeight` 为 0，
 * `SongList` 回退到 520px ⇒ 24 行），但滚远之后一定离开窗口。两次按压命中的是
 * 同一个 song，却是两个不同的 DOM 节点。
 */
function windowedSongs() {
  const songs = Array.from({ length: 120 }, (_, i) => makeSong(`song-${i}`, `Track ${i}`));
  songs.splice(5, 0, makeSong("song-40", "Windowed Away"));
  return songs;
}

describe("触发控件 toggle（fix-queue-trigger-toggle）", () => {
  afterEach(() => {
    // @ts-expect-error test hook
    globalThis.__echoTest?.reset();
  });

  it("再次点击打开菜单的同一行会关闭它", async () => {
    await renderWorkspace();
    const more = screen.getAllByRole("button", { name: "歌曲操作" })[0];

    press(more);
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    // The row counts as interior, so the stack does not dismiss on the press —
    // the click's toggle is the only thing that can close the menu.
    press(more);
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });

  it("第二次点击仍会切换到另一行的菜单，而不是直接关掉", async () => {
    await renderWorkspace();
    const [first, second] = screen.getAllByRole("button", { name: "歌曲操作" });
    press(first);
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    // A different row is a different trigger, so this press opens that row's
    // menu rather than closing the one on screen.
    press(second);
    await waitFor(() => {
      const menu = screen.getByRole("menu", { name: "歌曲操作菜单" });
      expect(menu).toHaveTextContent("Velvet Night");
    });

    press(second);
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });

  it("菜单外、触发行外的按压仍然收起菜单", async () => {
    await renderWorkspace();
    press(screen.getAllByRole("button", { name: "歌曲操作" })[0]);
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    // The dismissal this change must not break: a press on genuinely outside
    // territory closes the menu.
    fireEvent.pointerDown(document.body);
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });

  it("多选状态下再次右键同一行按当前选集重新打开批量菜单", async () => {
    await renderWorkspace();

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    fireEvent.click(screen.getByTestId("song-select-song-1"));
    const row = screen.getByTestId("song-row-song-1");

    // In multi-select the row's secondary press opens the batch menu. The batch
    // menu is deliberately not a toggle: the press is interior, so the stack
    // does not dismiss it, and the handler re-opens it for the selection.
    rightPress(row);
    expect(await screen.findByTestId("batch-song-menu")).toHaveTextContent("已选 1 首歌曲");

    fireEvent.click(screen.getByTestId("song-select-song-2"));
    rightPress(row);
    await waitFor(() =>
      expect(screen.getByTestId("batch-song-menu")).toHaveTextContent("已选 2 首歌曲"),
    );
  });

  it("非多选状态下右键同一行会切换单歌曲菜单", async () => {
    await renderWorkspace();
    const row = screen.getByTestId("song-row-song-1");

    rightPress(row);
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    // Outside multi-select the row's secondary press drives the single-song
    // menu, which does toggle.
    rightPress(row);
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });

  it("右键打开的菜单仍由面板外按压和 Escape 收起", async () => {
    // 触发控件计入「内部区域」只约束打开它的那个入口，不许把菜单整体变成
    // 按不掉的浮层。这条路必须仍由菜单自己原有的规则收拾。
    await renderWorkspace();
    rightPress(screen.getByTestId("song-row-song-1"));
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    // 面板外部的按压仍然收起菜单，即使打开它的手势来自右键。
    fireEvent.pointerDown(document.body);
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );

    // Escape 同理：它由单一浮层栈关闭顶层，与触发控件无关。
    rightPress(screen.getByTestId("song-row-song-1"));
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });

  it("入口控件在窗口化回收并重建之后，再次按压仍然关闭菜单", async () => {
    // 这是把"哪个入口"写成 DOM 节点身份时漏掉的那个场景：列表窗口化，滚出
    // 可视区的行会被回收，滚回来时是**新的** `<tr>` 和新的按钮。菜单仍然开着，
    // 所以按下的还是同一个入口 —— 归属必须靠 song.id 判，不能靠节点。
    await renderWorkspace(windowedSongs());

    const before = screen.getByTestId("song-row-song-40");
    press(within(before).getByRole("button", { name: "歌曲操作" }));
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    // Far enough that the row leaves the render window, then back to it.
    const viewport = screen.getByTestId("song-list");
    fireEvent.scroll(viewport, { target: { scrollTop: 4000 } });
    await waitFor(() => expect(screen.queryByTestId("song-row-song-40")).not.toBeInTheDocument());
    fireEvent.scroll(viewport, { target: { scrollTop: 0 } });
    const after = await screen.findByTestId("song-row-song-40");
    expect(after).not.toBe(before);

    press(within(after).getByRole("button", { name: "歌曲操作" }));
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });

  it("入口行在窗口化回收并重建之后，再次右键仍然切换菜单", async () => {
    // Same story for the right-click path, whose trigger is the row: the row
    // element recorded on the first press is a different object by the second.
    await renderWorkspace(windowedSongs());

    const before = screen.getByTestId("song-row-song-40");
    rightPress(before);
    expect(await screen.findByRole("menu", { name: "歌曲操作菜单" })).toBeInTheDocument();

    const viewport = screen.getByTestId("song-list");
    fireEvent.scroll(viewport, { target: { scrollTop: 4000 } });
    await waitFor(() => expect(screen.queryByTestId("song-row-song-40")).not.toBeInTheDocument());
    fireEvent.scroll(viewport, { target: { scrollTop: 0 } });
    const after = await screen.findByTestId("song-row-song-40");
    expect(after).not.toBe(before);

    rightPress(after);
    await waitFor(() =>
      expect(screen.queryByRole("menu", { name: "歌曲操作菜单" })).not.toBeInTheDocument(),
    );
  });
});
