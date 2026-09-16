/**
 * Playlist content view (task 10.9).
 *
 * A playlist is *the same workspace view* the prototype shows for 全部歌曲 —
 * `header.topbar` + `main.content` + `.library-view` (`.library-head` with the
 * playlist title, its member count and the view tools, then the shared
 * `.table-wrap` song table). It used to sit inside its own `.workspace` grid,
 * which is why it read as a different application.
 *
 * Playlist-level actions live in `.library-tools` as prototype `.tool-button`s
 * (编辑歌单 / 删除歌单): the prototype reaches 编辑歌单 through a context menu on
 * the sidebar item, and has no 删除歌单 at all — both are real release
 * capabilities (task 10.9), so they are surfaced with the prototype's own tool
 * button rather than inventing new chrome. Deleting a playlist never deletes a
 * song file.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { SongView } from "../../ipc/ipc-types.generated";
import { SongList } from "../library/SongList";
import { bumpLibraryCount, invalidateLibraryCounts } from "../library/coverPalette";
import { publishSongUpdate } from "../library/songUpdates";
import { ConfirmationDialog, SongMenu, type MenuAnchor } from "../library/SongMenu";
import { PlaylistNameDialog } from "./PlaylistNameDialog";
import { Icon } from "../../app/Icon";
import { Topbar } from "../../app/shell";
import { notify } from "../../app/toast";

export interface PlaylistsViewProps {
  readonly playlistId: string;
  /** The playlist's display name, resolved by the shell. */
  readonly title: string;
  readonly coverKey?: string;
  readonly hasCustomCover?: boolean;
  readonly root: string;
  /** Authoritative sibling names from the shell, for local rename validation. */
  readonly existingNames: readonly string[];
  readonly readOnly: boolean;
  /** The playlist no longer exists — the shell returns to 全部歌曲. */
  readonly onDeleted?: () => void;
  /** A playlist mutation landed; the shell re-reads the sidebar list. */
  readonly onLibraryChanged?: () => void;
}

export function PlaylistsView({
  playlistId,
  title,
  coverKey,
  hasCustomCover = false,
  root,
  existingNames,
  readOnly,
  onDeleted,
  onLibraryChanged,
}: PlaylistsViewProps) {
  const [members, setMembers] = useState<readonly SongView[]>([]);
  const [name, setName] = useState(title);
  const [menuFor, setMenuFor] = useState<{ song: SongView; anchor: MenuAnchor } | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const request = useRef(0);
  const snapshot = usePlayerSnapshot();

  // The shell owns the list; keep the optimistic name in step with it.
  useEffect(() => setName(title), [title]);

  const loadMembers = useCallback(() => {
    const id = ++request.current;
    void bridge
      .call("playlist_members", { playlistId })
      // A malformed payload degrades to an empty list instead of tearing down
      // the whole view (the IPC contract says this is always an array).
      .then((value: unknown) => {
        if (id !== request.current) return;
        if (Array.isArray(value)) {
          setMembers(value as SongView[]);
          setLoadError(null);
        }
      })
      .catch(() => {
        if (id === request.current) setLoadError("加载歌单失败，请重试");
      });
  }, [playlistId]);

  useEffect(loadMembers, [loadMembers]);

  async function deletePlaylist() {
    setConfirmDelete(false);
    try {
      await bridge.call("delete_playlist", { id: playlistId });
      invalidateLibraryCounts();
      onLibraryChanged?.();
      onDeleted?.();
    } catch {
      notify({ message: "删除歌单失败", error: true });
    }
  }

  const onPlay = useCallback(
    (song: SongView) => {
      const selectedIndex = members.findIndex((s) => s.id === song.id);
      void bridge.call("play_context", {
        songs: members.map((s) => s.id),
        selectedIndex: selectedIndex < 0 ? 0 : selectedIndex,
        source: `playlist:${playlistId}`,
      });
    },
    [members, playlistId],
  );

  const onFavorite = useCallback(
    (song: SongView, favorite: boolean) => {
      // Optimistic: the sidebar count moves with the click, not after it.
      bumpLibraryCount("favorites", favorite ? 1 : -1);
      void bridge
        .call("set_favorite", { songId: song.id, favorite })
        .then((committed) => {
          // Broadcast the committed view (player bar / other surfaces adopt
          // the new heart), then refresh the member list.
          publishSongUpdate(committed as SongView);
          loadMembers();
        })
        .catch(() => {
          bumpLibraryCount("favorites", favorite ? -1 : 1);
          notify({ message: "收藏失败，请重试", error: true });
        });
    },
    [loadMembers],
  );

  const onEnqueue = useCallback((song: SongView) => {
    void bridge
      .call("queue_command", { command: "enqueue", songId: song.id })
      .then(() => notify(`已将 ${song.title ?? "歌曲"} 加入播放队列`))
      .catch(() => notify({ message: "加入播放队列失败，请重试", error: true }));
  }, []);

  async function removeMember(song: SongView) {
    try {
      await bridge.call("remove_playlist_song", { playlist: playlistId, song: song.id });
      loadMembers();
      onLibraryChanged?.();
      setMenuFor(null);
    } catch {
      notify({ message: "移除歌曲失败，请重试", error: true });
    }
  }

  const unavailableCount = members.filter((s) => s.availability !== "available").length;

  return (
    <>
      <Topbar title={name} />

      <main className="content">
        <div className="library-view" data-testid="playlist-view">
          <div className="library-head">
            <div className="list-context">
              <h1 id="view-title" data-testid="view-title">
                {name}
              </h1>
              <span className="library-total">{members.length} 首</span>
            </div>
            <div className="library-tools">
              <button
                type="button"
                className="tool-button"
                disabled={readOnly}
                onClick={() => setRenaming(true)}
              >
                <Icon name="edit" />
                编辑歌单
              </button>
              <button
                type="button"
                className="tool-button"
                disabled={readOnly}
                onClick={() => setConfirmDelete(true)}
              >
                <Icon name="trash" />
                删除歌单
              </button>
            </div>
          </div>

          <p className="playlist-summary">
            共 {members.length} 首{unavailableCount > 0 ? `（${unavailableCount} 首不可用）` : ""}
          </p>
          {loadError ? (
            <button type="button" className="btn" onClick={loadMembers}>
              {loadError}
            </button>
          ) : null}

          <SongList
            songs={members}
            search=""
            loading={false}
            isLast
            readOnly={readOnly}
            currentSongId={snapshot.currentSongId}
            playing={snapshot.state === "playing"}
            onLoadMore={() => {}}
            onClearSearch={() => {}}
            onPlay={onPlay}
            onFavorite={onFavorite}
            onEnqueue={onEnqueue}
            onOpenMenu={(song, anchor) => setMenuFor({ song, anchor })}
          />
        </div>
      </main>

      {menuFor ? (
        <SongMenu
          song={menuFor.song}
          root={root}
          readOnly={readOnly}
          anchor={menuFor.anchor}
          onClose={() => setMenuFor(null)}
          onPlay={() => {
            onPlay(menuFor.song);
            setMenuFor(null);
          }}
          onPlayNext={() => {
            void bridge.call("queue_command", {
              command: "playNext",
              songId: menuFor.song.id,
            });
            setMenuFor(null);
          }}
          onEnqueue={() => {
            onEnqueue(menuFor.song);
            setMenuFor(null);
          }}
          onFavorite={(favorite) => {
            onFavorite(menuFor.song, favorite);
            setMenuFor(null);
          }}
          onRefresh={loadMembers}
          extraActions={
            <button
              type="button"
              className="menu-action danger"
              onClick={() => void removeMember(menuFor.song)}
            >
              <Icon name="trash" />
              从歌单移除
            </button>
          }
        />
      ) : null}

      {renaming ? (
        <PlaylistNameDialog
          mode="edit"
          playlistId={playlistId}
          initialName={name}
          initialCoverKey={coverKey}
          hasCustomCover={hasCustomCover}
          existingNames={existingNames}
          onClose={() => setRenaming(false)}
          onDone={(next) => {
            setName(next);
            onLibraryChanged?.();
          }}
        />
      ) : null}

      {confirmDelete ? (
        <ConfirmationDialog
          title={`删除歌单「${name}」？`}
          description="只会删除歌单本身，资料库中的歌曲文件不会被删除。"
          confirmLabel="确认删除歌单"
          onConfirm={() => void deletePlaylist()}
          onCancel={() => setConfirmDelete(false)}
        />
      ) : null}
    </>
  );
}
