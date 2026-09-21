/**
 * 歌曲操作菜单 (task 10.6 / 10.8) — the prototype's `.song-menu`.
 *
 * DOM, geometry and copy come from the prototype
 * (`docs/prototype/echo-desktop-player.html`): a fixed 248px popover holding a
 * `.menu-song` card and a stack of `.menu-action` rows, positioned with the
 * prototype's own clamp
 *   left = clamp(12, min(innerWidth − 260, trigger.right − 248))
 *   top  = clamp(12, min(innerHeight − 254, trigger.bottom + 6))
 * There is no scrim in the prototype — the menu closes on an outside pointer
 * press — so neither is there one here.
 *
 * The delete flow follows the prototype exactly:
 *  删除 → the 阻断确认 dialog (`.confirmation-dialog`) → on success the menu
 *  closes and the 撤销 affordance appears in the toast (`.toast` +
 *  `.toast-action`) with a real 10-second window. A failed delete keeps the menu
 *  open with an inline `.menu-error` — it never fabricates a deletion.
 *
 * Item set: the prototype's five actions (下一首播放 / 添加到歌单 / 显示歌曲详情 /
 * 打开本地目录 / 删除) plus 播放 / 加入播放队列 / 收藏, which this release exposes
 * as real capabilities (tasks 10.6 / 10.8). Every extra row uses the prototype's
 * `.menu-action` anatomy — nothing bespoke. 关闭 is intentionally absent: the
 * prototype has no such item, and Escape / outside-click already cover it.
 *
 * Decomposition (task 6.6): the anchored placement (`usePlacement`), the
 * blocking confirm dialog (`ConfirmationDialog`) and the read-only detail
 * (`SongDetail`) each live in their own module so this file keeps the menu's
 * item set, the reveal/delete flows and the surface markup (CODE_STANDARDS §6).
 */

import { useEffect, useRef, useState, type ReactNode } from "react";

import { assetUrl, bridge } from "../../bridge";
import { useCoverKeys } from "../../app/coverArt";
import { OverlayTier, useFocusTrap, useOverlay, useRovingFocus } from "../../app/overlays";
import { Icon } from "../../app/Icon";
import { notify } from "../../app/toast";
import type { SongView } from "../../ipc/ipc-types.generated";
import { coverClass } from "./coverClass";
import { invalidateLibraryCounts } from "./libraryCounts";
import { usePlacement } from "./usePlacement";
import { ConfirmationDialog } from "./ConfirmationDialog";
import { SongDetail } from "./SongDetail";

/** Viewport anchor for the control or pointer location that opened the menu. */
export interface MenuAnchor {
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
  readonly left: number;
  /** Context-menu anchors use the pointer location instead of a trigger box. */
  readonly kind?: "pointer";
}

export interface SongMenuProps {
  readonly song: SongView;
  readonly root: string;
  readonly readOnly: boolean;
  /** Where to anchor the popover; omitted falls back to the viewport corner. */
  readonly anchor?: MenuAnchor | null;
  readonly onClose: () => void;
  readonly onPlay: () => void;
  /** Insert this song right after the current one ("下一首播放", task 10.6). */
  readonly onPlayNext?: () => void;
  /** Append this song to the end of the queue ("加入播放队列", task 10.6). */
  readonly onEnqueue?: () => void;
  readonly onFavorite: (favorite: boolean) => void;
  readonly onRefresh: () => void;
  /** Open the add-to-playlist selector for this song (task 10.9). */
  readonly onAddToPlaylist?: () => void;
  readonly extraActions?: ReactNode;
}

/** The 撤销 window (task 10.8). */
const UNDO_WINDOW_MS = 10_000;

export function SongMenu({
  song,
  root,
  readOnly,
  anchor,
  onClose,
  onPlay,
  onPlayNext,
  onEnqueue,
  onFavorite,
  onRefresh,
  onAddToPlaylist,
  extraActions,
}: SongMenuProps) {
  const [detailOpen, setDetailOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const menuRef = useRef<HTMLElement>(null);
  // The menu shows the same artwork the row does. Asking for this one id is
  // free when the list already resolved it (the store de-duplicates) and is the
  // only source when the menu is opened from somewhere else.
  const coverKey = useCoverKeys([song.id]).get(song.id) || null;
  // Hook order is fixed: placement is computed before any early return, so the
  // menu never changes its hook count between renders.
  const style = usePlacement(anchor, menuRef);
  // Single overlay stack: the menu is a `Menu` tier layer; Escape closes it via
  // the global handler, focus is trapped, and focus returns to the row on close.
  useOverlay({ tier: OverlayTier.Menu, onClose, containerRef: menuRef });
  useFocusTrap(menuRef);

  // Outside press closes the menu, as in the prototype (which stops propagation
  // on the trigger instead of drawing a scrim). Suspended while a higher layer
  // owned by this component is open, so the press lands on that layer.
  const dismissable = !confirmDelete && !detailOpen;
  useEffect(() => {
    if (!dismissable) return;
    function onPointerDown(event: Event) {
      const target = event.target as Node | null;
      if (target && menuRef.current?.contains(target)) return;
      onClose();
    }
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [dismissable, onClose]);

  const title = song.title ?? "未命名歌曲";
  const artist = song.artist ?? "未知艺人";
  const unavailable = song.availability !== "available";

  async function reveal() {
    try {
      const result = await bridge.call("reveal_song", { songId: song.id });
      if (!result.revealed) {
        // Soft failure — show the relative path instead; never an absolute one.
        notify({
          message: `已在资料库中找到：${result.relativePath}（无法打开文件管理器）`,
        });
        onClose();
        return;
      }
      notify("已打开所在文件夹");
      onClose();
    } catch {
      setError("无法定位该文件");
    }
  }

  async function deleteSong() {
    setConfirmDelete(false);
    setError(null);
    try {
      const operation = await bridge.call("delete_song", {
        root,
        song: song.id,
      });
      onRefresh();
      // The song leaves every view's total, not just this one's rows.
      invalidateLibraryCounts();
      // The prototype removes the row and reports through the toast, whose
      // 撤销 action is the only way back. The window really closes at 10s.
      notify({
        message: `已将「${title}」移至回收站，${UNDO_WINDOW_MS / 1000} 秒内可撤销`,
        actionLabel: "撤销",
        autoDismissMs: UNDO_WINDOW_MS,
        onAction: () => {
          void bridge
            .call("undo_delete", { root, operation })
            .then(() => {
              notify(`已恢复「${title}」`);
              onRefresh();
              invalidateLibraryCounts();
            })
            .catch(() => notify({ message: "撤销失败或已超时", error: true }));
        },
      });
      onClose();
    } catch {
      setError("删除失败，请重试");
    }
  }

  // Roving-focus menu items (WAI-ARIA menu pattern). Hoisted above any early
  // return so the hook order stays stable.
  const items = [
    { id: "song-menu-play", label: "播放", icon: "play", run: () => onPlay() },
    {
      id: "song-menu-next",
      label: "下一首播放",
      icon: "playNext",
      run: () => onPlayNext?.(),
      disabled: !onPlayNext || unavailable,
    },
    {
      id: "song-menu-add-playlist",
      label: "添加到歌单",
      icon: "plus",
      run: () => onAddToPlaylist?.(),
      disabled: readOnly || !onAddToPlaylist || unavailable,
    },
    {
      id: "song-menu-detail",
      label: "显示歌曲详情",
      icon: "info",
      run: () => setDetailOpen(true),
    },
    {
      id: "song-menu-folder",
      label: "打开本地目录",
      icon: "folder",
      run: () => void reveal(),
      disabled: unavailable,
    },
    {
      id: "song-menu-enqueue",
      label: "加入播放队列",
      icon: "queue",
      run: () => onEnqueue?.(),
      disabled: !onEnqueue || unavailable,
    },
    {
      id: "song-menu-favorite",
      label: song.favorite ? "取消收藏" : "收藏",
      icon: "heart",
      run: () => onFavorite(!song.favorite),
      disabled: readOnly,
    },
    ...(readOnly
      ? []
      : [
          {
            id: "song-menu-delete",
            label: "删除",
            icon: "trash",
            run: () => {
              setError(null);
              setConfirmDelete(true);
            },
            danger: true,
          },
        ]),
  ].map((item) => ({ ...item, disabled: item.disabled ?? false }));
  const { onMenuKeyDown, setActive } = useRovingFocus(items);

  // The prototype focuses `#menu-next` when the menu opens, not the first row.
  // Declared after `useOverlay` so this effect runs after the overlay's own
  // "focus the first focusable" pass and wins.
  useEffect(() => {
    document.getElementById("song-menu-next")?.focus();
  }, []);

  if (confirmDelete) {
    // The prototype closes the menu first and shows only the confirmation.
    return (
      <ConfirmationDialog
        title={`删除「${title}」？`}
        description="歌曲会移至回收站。你仍可立即撤销本次操作。"
        confirmLabel="移至回收站"
        onConfirm={() => void deleteSong()}
        onCancel={() => setConfirmDelete(false)}
      />
    );
  }

  return (
    <>
      <section
        className="song-menu"
        role="menu"
        aria-label="歌曲操作菜单"
        ref={menuRef}
        onKeyDown={onMenuKeyDown}
        style={style}
        data-testid="song-menu"
      >
        <div className="menu-song">
          <div
            className={`cover ${coverClass(song.id)}${coverKey ? " has-image" : ""}`}
            aria-hidden="true"
          >
            {coverKey ? <img src={assetUrl(coverKey)} alt="" /> : null}
          </div>
          <div>
            <b id="menu-title">{title}</b>
            <span id="menu-artist">{artist}</span>
          </div>
        </div>

        {items.map((item) => (
          <button
            key={item.id}
            id={item.id}
            type="button"
            role="menuitem"
            className={`menu-action${item.danger ? " danger" : ""}`}
            disabled={item.disabled}
            onClick={() => {
              if (!item.disabled) item.run();
            }}
            onFocus={() => setActive(item.id)}
          >
            <Icon name={item.icon} filled={item.icon === "heart" && song.favorite} />
            {item.label}
          </button>
        ))}

        {error ? (
          <p className="menu-error" role="alert">
            {error}
          </p>
        ) : null}
        {extraActions}
      </section>

      {detailOpen ? (
        <SongDetail
          song={song}
          onClose={() => setDetailOpen(false)}
          onReveal={() => void reveal()}
        />
      ) : null}
    </>
  );
}
