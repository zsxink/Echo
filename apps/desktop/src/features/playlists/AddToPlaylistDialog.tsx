/**
 * 添加到歌单选择器 (task 10.9) — the prototype's `.playlist-picker-dialog`.
 *
 * Reproduces the prototype's panel exactly: `.playlist-picker-head` title,
 * `.playlist-picker-sub` ("将「歌曲」添加到："), a scrollable
 * `.playlist-picker-list` of `.playlist-picker-option` rows (cover + name +
 * member count + the round `.playlist-picker-check`), and a footer with
 * `.playlist-picker-new` + 取消 / 确认.
 *
 * The commit sends **one** `add_to_playlists(song, targets)` mutation so a user
 * adding a song to several playlists cannot half-succeed; duplicate membership is
 * idempotent server-side (task 6.6). Cancel never mutates.
 *
 * Deviation from the prototype: its picker pre-selects the song's current
 * memberships and commits a diff (adds *and* removals). The release's command
 * contract for this surface is additive — removal lives in the playlist view
 * (`remove_playlist_song`) — so the options start unselected here rather than
 * implying a removal that would silently not happen.
 */

import { useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { Icon } from "../../app/Icon";
import { notify } from "../../app/toast";
import type { PlaylistView, SongView } from "../../ipc/ipc-types.generated";
import { coverClass, formatBatchResult, runAddToPlaylistsBatch } from "../library";
import { PlaylistNameDialog } from "./PlaylistNameDialog";

export interface AddToPlaylistDialogProps {
  /** Backward-compatible single-song entry point. */
  readonly songId?: string;
  /** Batch entry point; ids are submitted in display order. */
  readonly songIds?: readonly string[];
  /** Optional authoritative views for batch eligibility and result details. */
  readonly songs?: readonly SongView[];
  /** Shown in the picker's sub line; the song's display title. */
  readonly songTitle?: string;
  /** The active root is required if the picker creates a playlist inline. */
  readonly root?: string;
  /** Effective library write capability, including the startup recovery gate. */
  readonly readOnly?: boolean;
  readonly onClose: () => void;
  readonly onDone: () => void;
}

export function AddToPlaylistDialog({
  songId,
  songIds,
  songs,
  songTitle,
  root,
  readOnly = false,
  onClose,
  onDone,
}: AddToPlaylistDialogProps) {
  const selectedSongIds = songs?.map((song) => song.id) ?? songIds ?? (songId ? [songId] : []);
  const [playlists, setPlaylists] = useState<readonly PlaylistView[]>([]);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const dialogRef = useRef<HTMLElement>(null);
  // A `Picker`-tier dialog (设置/歌单选择器): Escape closes it via the single
  // stack, focus is trapped and restored on close.
  useOverlay({ tier: OverlayTier.Picker, onClose, containerRef: dialogRef, enabled: !creating });
  useFocusTrap(dialogRef, !creating);

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
    if (selected.size === 0 || selectedSongIds.length === 0) {
      setError("请至少选择一个歌单");
      return;
    }
    setError(null);
    try {
      const operationSongs =
        songs ??
        selectedSongIds.map((id): SongView => ({
          id,
          favorite: false,
          playCount: 0,
          availability: "available",
          relativePath: "",
        }));
      const result = await runAddToPlaylistsBatch(operationSongs, Array.from(selected));
      if (result.failed > 0) {
        setError(
          selectedSongIds.length === 1
            ? "添加失败，请重试"
            : `部分歌曲添加失败（${formatBatchResult(result)}）`,
        );
        return;
      }
      if (result.skipped > 0 || selectedSongIds.length > 1) {
        notify(`添加到歌单：${formatBatchResult(result)}`);
      }
      if (result.succeeded > 0 || result.skipped > 0) {
        onDone();
        onClose();
      }
    } catch {
      setError("添加失败，请重试");
    }
  }

  return (
    <>
      <section
        className="playlist-picker-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="playlist-picker-title"
        data-testid="add-to-playlist-dialog"
        ref={dialogRef}
      >
        <div className="playlist-picker-panel">
          <div className="playlist-picker-head">
            <h2 id="playlist-picker-title">添加到歌单</h2>
          </div>
          <p className="playlist-picker-sub" id="playlist-picker-sub">
            {selectedSongIds.length === 1
              ? `将「${songTitle ?? "歌曲"}」添加到：`
              : `将已选 ${selectedSongIds.length} 首歌曲添加到：`}
          </p>

          <div className="playlist-picker-list" role="listbox" aria-label="选择歌单">
            {playlists.length === 0 ? (
              <div className="playlist-picker-empty">
                <p>还没有歌单，先创建一个吧。</p>
              </div>
            ) : (
              playlists.map((playlist) => {
                const added = selected.has(playlist.id);
                return (
                  <button
                    key={playlist.id}
                    type="button"
                    role="option"
                    aria-selected={added}
                    aria-label={playlist.name}
                    className={`playlist-picker-option${added ? " added" : ""}`}
                    onClick={() => {
                      toggle(playlist.id);
                      setError(null);
                    }}
                  >
                    <span
                      className={`cover ${coverClass(playlist.id)}${playlist.coverKey ? " has-image" : ""}`}
                      aria-hidden="true"
                    >
                      {playlist.coverKey ? (
                        <img src={bridge.assetUrl(playlist.coverKey)} alt="" />
                      ) : null}
                    </span>
                    <span className="playlist-picker-copy">
                      <strong>{playlist.name}</strong>
                      <span>{playlist.memberCount} 首歌曲</span>
                    </span>
                    <span className="playlist-picker-check" aria-hidden="true">
                      <Icon name="check" />
                    </span>
                  </button>
                );
              })
            )}
          </div>

          {error ? (
            <p className="playlist-name-error" role="alert">
              {error}
            </p>
          ) : null}

          <div className="playlist-picker-footer">
            <button
              type="button"
              className="playlist-picker-new"
              disabled={readOnly}
              title={readOnly ? "当前资料库为只读状态" : undefined}
              onClick={() => setCreating(true)}
              data-testid="playlist-picker-new"
            >
              <Icon name="plus" />
              新建歌单
            </button>
            <div className="playlist-picker-footer-actions">
              <button type="button" className="playlist-picker-cancel" onClick={onClose}>
                取消
              </button>
              <button
                type="button"
                className="playlist-picker-confirm"
                disabled={readOnly}
                onClick={() => void confirm()}
              >
                确认
              </button>
            </div>
          </div>
        </div>
      </section>

      {creating ? (
        <PlaylistNameDialog
          mode="create"
          root={root}
          existingNames={playlists.map((playlist) => playlist.name)}
          onClose={() => setCreating(false)}
          onDone={(name, createdId) => {
            // The create command returns the authoritative id. Keep the picker
            // mounted, refresh its list, and select that id so confirmation can
            // immediately add the original song without making the user find it
            // again. A re-read also supplies the updated sidebar count upstream.
            void bridge
              .call("playlists")
              .then((value: unknown) => {
                setPlaylists(value as PlaylistView[]);
                if (createdId) {
                  setSelected((previous) => new Set(previous).add(createdId));
                }
              })
              .catch(() => setError("歌单已创建，但列表刷新失败，请重试"));
            setCreating(false);
            setError(createdId ? null : `已创建歌单「${name}」，请选择它`);
          }}
        />
      ) : null}
    </>
  );
}
