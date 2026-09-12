/**
 * Add-to-playlist selector (task 10.9).
 *
 * A multi-select that lists every playlist and lets the user add the song to
 * several at once via the single `add_to_playlists(song, targets)` mutation.
 * Duplicate membership is idempotent (the core rejects a repeat member, task
 * 6.6), and the list is re-fetched so the sidebar/playlist counts stay in sync.
 * A cancel never mutates anything.
 */

import { useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import type { PlaylistView } from "../../ipc/ipc-types.generated";

export interface AddToPlaylistDialogProps {
  readonly songId: string;
  readonly onClose: () => void;
  readonly onDone: () => void;
}

export function AddToPlaylistDialog({ songId, onClose, onDone }: AddToPlaylistDialogProps) {
  const [playlists, setPlaylists] = useState<readonly PlaylistView[]>([]);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  // A `Picker`-tier dialog (设置/歌单选择器): Escape closes it via the single
  // stack, focus is trapped and restored on close.
  useOverlay({ tier: OverlayTier.Picker, onClose, containerRef: dialogRef });
  useFocusTrap(dialogRef);

  useEffect(() => {
    let cancelled = false;
    void bridge
      .call("playlists")
      .then((value: unknown) => {
        if (!cancelled) setPlaylists(value as PlaylistView[]);
      })
      .catch(() => {
        if (!cancelled) setError("无法加载歌单");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  async function confirm() {
    if (selected.size === 0) {
      setError("请至少选择一个歌单");
      return;
    }
    setError(null);
    try {
      await bridge.call("add_to_playlists", {
        song: songId,
        targets: Array.from(selected),
      });
      onDone();
      onClose();
    } catch (err) {
      // A conflict (already a member) is idempotent by design; only surface a
      // non-idempotent failure.
      if (codeOf(err) !== "conflict") {
        setError("添加失败，请重试");
      } else {
        onDone();
        onClose();
      }
    }
  }

  return (
    <div className="overlay-shell" data-testid="add-to-playlist-dialog" onClick={onClose}>
      <div
        className="detail-card"
        role="dialog"
        aria-modal="true"
        aria-label="加入歌单"
        ref={dialogRef}
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="detail-title">加入歌单</h3>
        {playlists.length === 0 ? (
          <p className="workspace-hint">暂无歌单可添加</p>
        ) : (
          <ul className="playlist-picker" role="group" aria-label="选择歌单">
            {playlists.map((playlist) => (
              <li key={playlist.id}>
                <label className="playlist-picker-item">
                  <input
                    type="checkbox"
                    checked={selected.has(playlist.id)}
                    onChange={() => toggle(playlist.id)}
                    aria-label={playlist.name}
                  />
                  <span>{playlist.name}</span>
                </label>
              </li>
            ))}
          </ul>
        )}
        {error ? (
          <p className="workspace-error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="menu-actions">
          <button type="button" className="btn btn-primary" onClick={() => void confirm()}>
            添加
          </button>
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
        </div>
      </div>
    </div>
  );
}

function codeOf(err: unknown): string {
  if (err instanceof Error && "code" in err) {
    return (err as unknown as { code?: string }).code ?? "";
  }
  return "";
}
