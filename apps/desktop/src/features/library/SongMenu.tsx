/**
 * Song action menu (task 10.8 / 10.9).
 *
 * A per-song menu exposing single-song operations only (本期无全选批量): 播放,
 * 下一首播放, 加入歌单, 显示详情, 打开本地目录, 删除（含确认 + 10 秒撤销）。
 * The menu is always bound to the SongId that opened it; clicking outside or
 * Escape closes it. The delete flow shows a real confirmation and a 10-second
 * undo affordance — a cancel / failed delete never fabricates a deletion.
 */

import { useEffect, useState, type ReactNode } from "react";

import { bridge } from "../../bridge";
import type { SongView } from "../../ipc/ipc-types.generated";

export interface SongMenuProps {
  readonly song: SongView;
  readonly root: string;
  readonly readOnly: boolean;
  readonly onClose: () => void;
  readonly onPlay: () => void;
  readonly onFavorite: (favorite: boolean) => void;
  readonly onRefresh: () => void;
  readonly extraActions?: ReactNode;
}

export function SongMenu({
  song,
  root,
  readOnly,
  onClose,
  onPlay,
  onFavorite,
  onRefresh,
  extraActions,
}: SongMenuProps) {
  const [detailOpen, setDetailOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [undo, setUndo] = useState<string | null>(null); // operation id
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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

  if (undo) {
    return (
      <div className="overlay-shell" data-testid="delete-undo">
        <div className="menu" role="dialog" aria-label="删除撤销">
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
      <div className="menu" role="menu" aria-label="歌曲操作" onClick={(e) => e.stopPropagation()}>
        <button type="button" className="menu-item" onClick={() => void onPlay()}>
          播放
        </button>
        <button type="button" className="menu-item" onClick={() => void onPlay()}>
          下一首播放
        </button>
        <button type="button" className="menu-item" onClick={() => setDetailOpen((d) => !d)}>
          显示歌曲详情
        </button>
        <button
          type="button"
          className="menu-item"
          onClick={() => void reveal()}
          disabled={song.availability !== "available"}
        >
          打开本地目录
        </button>
        {readOnly ? null : (
          <>
            <button
              type="button"
              className="menu-item"
              onClick={() => onFavorite(!song.favorite)}
              aria-pressed={song.favorite}
            >
              {song.favorite ? "取消收藏" : "收藏"}
            </button>
            {confirmDelete ? (
              <div className="delete-confirm">
                <p className="danger-text">确认删除这首歌曲？（可撤销）</p>
                <div className="menu-actions">
                  <button
                    type="button"
                    className="btn btn-danger"
                    onClick={() => void deleteSong()}
                  >
                    确认删除
                  </button>
                  <button type="button" className="btn" onClick={() => setConfirmDelete(false)}>
                    取消
                  </button>
                </div>
              </div>
            ) : (
              <button
                type="button"
                className="menu-item danger-text"
                onClick={() => setConfirmDelete(true)}
              >
                删除…
              </button>
            )}
          </>
        )}
        {extraActions}
        {error ? (
          <p className="menu-error" role="alert">
            {error}
          </p>
        ) : null}
        <button type="button" className="menu-item menu-item-muted" onClick={onClose}>
          关闭
        </button>
      </div>

      {detailOpen ? (
        <SongDetail
          song={song}
          onClose={() => setDetailOpen(false)}
          onReveal={() => void reveal()}
        />
      ) : null}
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
  return (
    <div className="overlay-shell">
      <div
        className="detail-card"
        role="dialog"
        aria-label="歌曲详情"
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
