import { fireEvent, render, screen } from "@testing-library/react";
import { useRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";

import type { SongView } from "../../ipc/ipc-types.generated";
import { BatchSongMenu, type BatchSongActionHandlers } from "./BatchSongActions";

function song(id: string, favorite = false): SongView {
  return {
    id,
    title: id,
    favorite,
    playCount: 0,
    availability: "available",
    relativePath: `${id}.flac`,
  };
}

function handlers(): BatchSongActionHandlers {
  return {
    onFavorite: vi.fn(),
    onAddToPlaylist: vi.fn(),
    onPlayNext: vi.fn(),
    onEnqueue: vi.fn(),
    onDelete: vi.fn(),
    onRemoveFromPlaylist: vi.fn(),
  };
}

describe("BatchSongActions", () => {
  it("renders the batch action set in the context menu", () => {
    const actionHandlers = handlers();
    render(
      <BatchSongMenu
        songs={[song("a"), song("b", true)]}
        readOnly={false}
        inPlaylist
        anchor={{ top: 0, right: 300, bottom: 24, left: 0 }}
        handlers={actionHandlers}
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByText("已选 2 首歌曲")).toBeInTheDocument();
    expect(screen.getByText("加入歌单")).toBeInTheDocument();
    expect(screen.getByText("从歌单移除")).toBeInTheDocument();
    expect(screen.getByText("删除")).toBeInTheDocument();
    expect(screen.queryByText("显示歌曲详情")).toBeNull();
    expect(screen.queryByText("打开本地目录")).toBeNull();
  });

  it("disables write actions in a read-only surface but preserves queue actions", () => {
    const actionHandlers = handlers();
    render(
      <BatchSongMenu
        songs={[song("a"), song("b", true)]}
        readOnly
        inPlaylist
        handlers={actionHandlers}
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByTestId("batch-song-menu")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "加入歌单" })).toBeDisabled();
    expect(screen.getByRole("menuitem", { name: "从歌单移除" })).toBeDisabled();
    expect(screen.queryByRole("menuitem", { name: "删除" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "加入播放队列" })).toBeEnabled();
  });

  it("dispatches every enabled action to its owning surface", () => {
    const actionHandlers = handlers();
    render(
      <BatchSongMenu
        songs={[song("a"), song("b", true)]}
        readOnly={false}
        inPlaylist
        handlers={actionHandlers}
        onClose={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId("batch-favorite"));
    fireEvent.click(screen.getByTestId("batch-unfavorite"));
    fireEvent.click(screen.getByTestId("batch-add-playlist"));
    fireEvent.click(screen.getByTestId("batch-play-next"));
    fireEvent.click(screen.getByTestId("batch-enqueue"));
    fireEvent.click(screen.getByTestId("batch-remove-playlist"));
    fireEvent.click(screen.getByTestId("batch-delete"));

    expect(actionHandlers.onFavorite).toHaveBeenNthCalledWith(1, true);
    expect(actionHandlers.onFavorite).toHaveBeenNthCalledWith(2, false);
    expect(actionHandlers.onAddToPlaylist).toHaveBeenCalledTimes(1);
    expect(actionHandlers.onPlayNext).toHaveBeenCalledTimes(1);
    expect(actionHandlers.onEnqueue).toHaveBeenCalledTimes(1);
    expect(actionHandlers.onRemoveFromPlaylist).toHaveBeenCalledTimes(1);
    expect(actionHandlers.onDelete).toHaveBeenCalledTimes(1);
  });

  it("places a pointer-opened menu at the context-menu coordinates", () => {
    render(
      <BatchSongMenu
        songs={[song("a")]}
        readOnly={false}
        inPlaylist={false}
        anchor={{ top: 230, right: 120, bottom: 230, left: 120, kind: "pointer" }}
        handlers={handlers()}
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByTestId("batch-song-menu")).toHaveStyle({ left: "120px", top: "230px" });
  });

  it("dismisses on Escape and restores focus to the trigger", () => {
    const onClose = vi.fn();
    render(
      <>
        <button type="button" data-testid="trigger">
          trigger
        </button>
        <BatchSongMenu
          songs={[song("a")]}
          readOnly={false}
          inPlaylist={false}
          handlers={handlers()}
          onClose={onClose}
        />
      </>,
    );
    const trigger = screen.getByTestId("trigger");
    trigger.focus();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(trigger);
  });
});

/**
 * fix-queue-trigger-toggle — the popover is a sibling of the table, so without
 * `triggerRef` a press on the row that opened the menu dismissed it out from
 * under the very call that was about to re-open it for the current selection.
 *
 * Note the asymmetry with the single-song menu: the batch menu is deliberately
 * NOT a toggle. A second press re-opens it for the current selection, which is
 * the contract the multi-select spec sets out. These tests pin the *interior*
 * behaviour only — the selection-set rules are proven at host level in
 * `batchLibraryOperations.test.tsx`.
 */
describe("BatchSongMenu trigger control (fix-queue-trigger-toggle)", () => {
  function RowWithTrigger({ onClose }: { readonly onClose: () => void }) {
    const triggerRef = useRef<HTMLElement | null>(null);
    // The real views open unconditionally: the row is interior, so the stack
    // does not dismiss, and this handler re-opens for the selection.
    const [open, setOpen] = useState(true);
    return (
      <div>
        <table>
          <tbody>
            <tr ref={(el) => (triggerRef.current = el)} data-testid="trigger-row">
              <td>
                <button type="button" data-testid="trigger-button" onClick={() => setOpen(true)}>
                  歌曲操作
                </button>
              </td>
            </tr>
          </tbody>
        </table>
        {open ? (
          <BatchSongMenu
            songs={[song("a")]}
            readOnly={false}
            inPlaylist={false}
            handlers={handlers()}
            triggerRef={triggerRef}
            onClose={() => {
              setOpen(false);
              onClose();
            }}
          />
        ) : null}
      </div>
    );
  }

  it("treats a press on the row as interior, so the menu survives the gesture", () => {
    const onClose = vi.fn();
    render(<RowWithTrigger onClose={onClose} />);
    const trigger = screen.getByTestId("trigger-button");
    expect(screen.getByTestId("batch-song-menu")).toBeInTheDocument();

    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);

    // Without `triggerRef` the pointerdown would dismiss and `onClose` would
    // fire; the menu would only be back because the click re-opens it, which is
    // exactly the 先关后开 churn this change removes.
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId("batch-song-menu")).toBeInTheDocument();
  });

  it("still dismisses on a press outside both the menu and the row", () => {
    const onClose = vi.fn();
    render(<RowWithTrigger onClose={onClose} />);

    fireEvent.pointerDown(document.body);
    expect(onClose).toHaveBeenCalled();
  });
});
