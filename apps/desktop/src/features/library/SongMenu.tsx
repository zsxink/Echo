/**
 * Song action menu (task 10.8 / 10.9).
 *
 * A per-song menu exposing single-song operations only (本期无全选批量): 播放,
 * 下一首播放, 加入歌单, 显示详情, 打开本地目录, 删除（含确认 + 10 秒撤销）。
 * The menu is always bound to the SongId that opened it; clicking outside or
 * Escape closes it. The delete flow shows a real confirmation and a 10-second
 * undo affordance — a cancel / failed delete never fabricates a deletion.
 */

import { useRef, useState, type ReactNode } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay, useRovingFocus } from "../../app/overlays";
import type { SongView } from "../../ipc/ipc-types.generated";

export interface SongMenuProps {
  readonly song: SongView;
  readonly root: string;
  readonly readOnly: boolean;
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

export function SongMenu({
  song,
  root,
  readOnly,
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
  const [undo, setUndo] = useState<string | null>(null); // operation id
  const [error, setError] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  // Single overlay stack: the menu is a `Menu` tier layer; Escape closes it via
  // the global handler, focus is trapped, and focus returns to the row on close.
  useOverlay({ tier: OverlayTier.Menu, onClose, containerRef: menuRef });
  useFocusTrap(menuRef);

  async function reveal() {
    try {
      const result = (await bridge.call("reveal_song", { songId: song.id })) as {
        relativePath: string;
        revealed: boolean;
      };
      if (!result.revealed) {
        // Soft failure — show the relative path instead; never absolute.
        setError(`已在资料库中找到：${result.relativePath}（无法打开文件管理器）`);
      }
      onClose();
    } catch {
      setError("无法定位该文件");
    }
  }

  async function deleteSong() {
    setConfirmDelete(false);
    setError(null);
    try {
      const operation = (await bridge.call("delete_song", {
        root,
        song: song.id,
      })) as string;
      setUndo(operation);
      onRefresh();
    } catch (err) {
      setError("删除失败，请重试");
      void err;
    }
  }

  async function undoDelete() {
    if (!undo) return;
    try {
      await bridge.call("undo_delete", { root, operation: undo });
      setUndo(null);
      onRefresh();
    } catch {
      setError("撤销失败或已超时");
      setUndo(null);
    }
  }

  // Roving-focus menu items (WAI-ARIA menu pattern). The invoking action id is
  // stable so the active item can be tracked and focused with arrow keys. This
  // must be hoisted above any early return to keep the hook order stable.
  const items = [
    { id: "song-menu-play", label: "播放", run: () => onPlay() },
    {
      id: "song-menu-play-next",
      label: "下一首播放",
      run: () => onPlayNext?.(),
      disabled: !onPlayNext || song.availability !== "available",
    },
    {
      id: "song-menu-enqueue",
      label: "加入播放队列",
      run: () => onEnqueue?.(),
      disabled: !onEnqueue || song.availability !== "available",
    },
    { id: "song-menu-detail", label: "显示歌曲详情", run: () => setDetailOpen(true) },
    {
      id: "song-menu-reveal",
      label: "打开本地目录",
      run: () => void reveal(),
      disabled: song.availability !== "available",
    },
    ...(readOnly
      ? []
      : [
          {
            id: "song-menu-add-to-playlist",
            label: "加入歌单…",
            run: () => onAddToPlaylist?.(),
            disabled: !onAddToPlaylist || song.availability !== "available",
          },
          {
            id: "song-menu-favorite",
            label: song.favorite ? "取消收藏" : "收藏",
            run: () => onFavorite(!song.favorite),
          },
          {
            id: "song-menu-delete",
            label: "删除…",
            run: () => setConfirmDelete(true),
            danger: true,
          },
        ]),
    { id: "song-menu-close", label: "关闭", run: onClose, muted: true },
  ].map((it) => ({ ...it, disabled: it.disabled ?? false }));
  const { activeId, onMenuKeyDown, setActive } = useRovingFocus(items);

  if (undo) {
    return (
      <div className="overlay-shell" data-testid="delete-undo">
        <div className="menu" role="dialog" aria-modal="true" aria-label="删除撤销">
          <p>歌曲已放入回收站，10 秒内可撤销。</p>
          <div className="menu-actions">
            <button type="button" className="btn btn-primary" onClick={() => void undoDelete()}>
              撤销
            </button>
            <button type="button" className="btn" onClick={onClose}>
              关闭
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="overlay-shell" data-testid="song-menu" onClick={onClose}>
      <div
        className="menu"
        role="menu"
        aria-label="歌曲操作"
        ref={menuRef}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onMenuKeyDown}
      >
        {items.map((item) => (
          <button
            key={item.id}
            id={item.id}
            type="button"
            role="menuitem"
            className={[
              "menu-item",
              item.danger ? "danger-text" : "",
              item.muted ? "menu-item-muted" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            disabled={item.disabled}
            onClick={() => {
              if (!item.disabled) item.run();
            }}
            onFocus={() => setActive(item.id)}
            aria-current={activeId === item.id ? "true" : undefined}
          >
            {item.label}
          </button>
        ))}
        {error ? (
          <p className="menu-error" role="alert">
            {error}
          </p>
        ) : null}
        {extraActions}
      </div>

      {detailOpen ? (
        <SongDetail
          song={song}
          onClose={() => setDetailOpen(false)}
          onReveal={() => void reveal()}
        />
      ) : null}

      {confirmDelete ? (
        <SongDeleteConfirm
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void deleteSong()}
        />
      ) : null}
    </div>
  );
}

/**
 * Blocking delete-confirmation dialog ("阻断确认/命名对话框" tier). Rendered as
 * a modal above the song menu; Escape closes it before the menu on the single
 * stack, and focus is trapped/restored.
 */
function SongDeleteConfirm({
  onCancel,
  onConfirm,
}: {
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useOverlay({ tier: OverlayTier.BlockingDialog, onClose: onCancel, containerRef: ref });
  useFocusTrap(ref);

  return (
    <div className="overlay-shell">
      <div
        className="detail-card"
        role="dialog"
        aria-modal="true"
        aria-label="确认删除"
        ref={ref}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="danger-text">确认删除这首歌曲？（可撤销）</p>
        <div className="menu-actions">
          <button type="button" className="btn btn-danger" onClick={onConfirm}>
            确认删除
          </button>
          <button type="button" className="btn" onClick={onCancel}>
            取消
          </button>
        </div>
      </div>
    </div>
  );
}

/** Read-only song detail (task 10.8): relative path only, never absolute. */
export function SongDetail({
  song,
  onClose,
  onReveal,
}: {
  song: SongView;
  onClose: () => void;
  onReveal: () => void;
}) {
  const detailRef = useRef<HTMLDivElement>(null);
  // A `Picker`-tier dialog above the menu: Escape closes it before the menu and
  // focus is trapped + restored.
  useOverlay({ tier: OverlayTier.Picker, onClose, containerRef: detailRef });
  useFocusTrap(detailRef);

  return (
    <div className="overlay-shell">
      <div
        className="detail-card"
        role="dialog"
        aria-modal="true"
        aria-label="歌曲详情"
        ref={detailRef}
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="detail-title">{song.title ?? "未命名歌曲"}</h3>
        <dl className="detail-grid">
          <dt>艺人</dt>
          <dd>{song.artist ?? "未知艺人"}</dd>
          <dt>专辑</dt>
          <dd>{song.album ?? "—"}</dd>
          <dt>资料库相对路径</dt>
          <dd data-testid="detail-relative-path">{song.relativePath}</dd>
          <dt>播放次数</dt>
          <dd>{song.playCount}</dd>
        </dl>
        <div className="menu-actions">
          <button type="button" className="btn" onClick={onReveal}>
            打开本地目录
          </button>
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
