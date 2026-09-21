import { useEffect, useRef } from "react";

import { OverlayTier, useFocusTrap, useOverlay, useRovingFocus } from "../../app/overlays";
import { Icon } from "../../app/Icon";
import type { SongView } from "../../ipc/ipc-types.generated";
import type { MenuAnchor } from "./SongMenu";
import { usePlacement } from "./usePlacement";

export interface BatchSongActionHandlers {
  readonly onFavorite: (favorite: boolean) => void;
  readonly onAddToPlaylist: () => void;
  readonly onPlayNext: () => void;
  readonly onEnqueue: () => void;
  readonly onDelete: () => void;
  readonly onRemoveFromPlaylist?: () => void;
}

export interface BatchSongActionOptions {
  readonly songs: readonly SongView[];
  readonly readOnly: boolean;
  readonly inPlaylist: boolean;
  readonly handlers: BatchSongActionHandlers;
}

export function SelectionModeButton({
  active,
  onToggle,
}: {
  readonly active: boolean;
  readonly onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className="tool-button tool-icon selection-mode-toggle"
      aria-label={active ? "退出多选" : "进入多选"}
      title={active ? "退出多选" : "进入多选"}
      aria-pressed={active}
      onClick={onToggle}
      data-testid="selection-mode-button"
    >
      <Icon name={active ? "close" : "selectAll"} />
    </button>
  );
}

interface BatchAction {
  readonly id: string;
  readonly label: string;
  readonly icon: string;
  readonly disabled?: boolean;
  readonly danger?: boolean;
  readonly run: () => void;
}

export function BatchSongMenu({
  anchor,
  songs,
  readOnly,
  inPlaylist,
  handlers,
  onClose,
}: BatchSongActionOptions & {
  readonly anchor?: MenuAnchor | null;
  readonly onClose: () => void;
}) {
  const menuRef = useRef<HTMLElement>(null);
  const style = usePlacement(anchor, menuRef);
  useOverlay({ tier: OverlayTier.Menu, onClose, containerRef: menuRef });
  useFocusTrap(menuRef);

  const actions = batchActions({ songs, readOnly, inPlaylist, handlers });
  const { onMenuKeyDown, setActive } = useRovingFocus(actions);
  const firstActionId = actions[0]?.id;

  useEffect(() => {
    if (firstActionId) document.getElementById(firstActionId)?.focus();
  }, [firstActionId]);

  return (
    <section
      className="song-menu batch-song-menu"
      role="menu"
      aria-label="批量歌曲操作菜单"
      ref={menuRef}
      style={style}
      onKeyDown={onMenuKeyDown}
      data-testid="batch-song-menu"
    >
      <div className="batch-menu-head">
        <strong>已选 {songs.length} 首歌曲</strong>
        <span>批量操作</span>
      </div>
      {actions.map((action) => (
        <button
          key={action.id}
          id={action.id}
          type="button"
          role="menuitem"
          className={`menu-action${action.danger ? " danger" : ""}`}
          disabled={action.disabled}
          data-testid={action.id}
          onClick={(event) => {
            // Close the transient menu before handing control to the owning
            // surface. This keeps the menu from intercepting the next click
            // when an action opens another dialog (for example, 加入歌单).
            event.stopPropagation();
            if (!action.disabled) action.run();
            onClose();
          }}
          onFocus={() => setActive(action.id)}
        >
          <Icon name={action.icon} />
          {action.label}
        </button>
      ))}
    </section>
  );
}

function batchActions({
  songs,
  readOnly,
  inPlaylist,
  handlers,
}: BatchSongActionOptions): BatchAction[] {
  const allFavorite = songs.length > 0 && songs.every((song) => song.favorite);
  const noneFavorite = songs.every((song) => !song.favorite);
  const actions: BatchAction[] = [
    {
      id: "batch-favorite",
      label: "收藏",
      icon: "heart",
      disabled: readOnly || allFavorite,
      run: () => handlers.onFavorite(true),
    },
    {
      id: "batch-unfavorite",
      label: "取消收藏",
      icon: "heart",
      disabled: readOnly || noneFavorite,
      run: () => handlers.onFavorite(false),
    },
    {
      id: "batch-add-playlist",
      label: "加入歌单",
      icon: "plus",
      disabled: readOnly,
      run: handlers.onAddToPlaylist,
    },
    {
      id: "batch-play-next",
      label: "下一首播放",
      icon: "playNext",
      run: handlers.onPlayNext,
    },
    {
      id: "batch-enqueue",
      label: "加入播放队列",
      icon: "queue",
      run: handlers.onEnqueue,
    },
  ];
  if (inPlaylist && handlers.onRemoveFromPlaylist) {
    actions.push({
      id: "batch-remove-playlist",
      label: "从歌单移除",
      icon: "trash",
      disabled: readOnly,
      run: handlers.onRemoveFromPlaylist,
    });
  }
  if (!readOnly) {
    actions.push({
      id: "batch-delete",
      label: "删除",
      icon: "trash",
      danger: true,
      run: handlers.onDelete,
    });
  }
  return actions;
}

export type { BatchAction };
