/**
 * Playlist content view (task 10.9).
 *
 * Shows a playlist's members in append order with the member count and
 * per-song actions, plus create / rename / delete for the playlist and add /
 * remove members. Invalid names (>40 graphemes, blank, duplicate) are rejected
 * with a per-field message; deleting a playlist never deletes song files.
 */

import { useCallback, useEffect, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { SongView } from "../../ipc/ipc-types.generated";
import { SongList } from "../library/SongList";
import { SongMenu } from "../library/SongMenu";

export function PlaylistsView({
  playlistId,
  onDeleted,
}: {
  playlistId: string;
  onDeleted?: () => void;
}) {
  const [members, setMembers] = useState<readonly SongView[]>([]);
  const [newName, setNewName] = useState("");
  const [renameValue, setRenameValue] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<SongView | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const snapshot = usePlayerSnapshot();

  const loadMembers = useCallback(() => {
    void bridge
      .call("playlist_members", { playlistId })
      .then((value: unknown) => setMembers(value as SongView[]))
      .catch(() => setMembers([]));
  }, [playlistId]);

  useEffect(loadMembers, [loadMembers]);

  // Resolve this playlist's current name (from the list) for the rename field.
  useEffect(() => {
    let cancelled = false;
    void bridge
      .call("playlists")
      .then((value: unknown) => {
        if (cancelled) return;
        const found = (value as { id: string; name: string }[]).find((p) => p.id === playlistId);
        if (found) setName(found.name);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [playlistId]);

  async function createPlaylist() {
    const trimmed = newName.trim();
    if (!trimmed) {
      setError("名称不能为空");
      return;
    }
    const graphemes = countGraphemes(trimmed);
    if (graphemes > 40) {
      setError(`名称不能超过 40 个字符（当前 ${graphemes} 个）`);
      return;
    }
    try {
      await bridge.call("create_playlist", { root: activeRoot(), name: trimmed });
      setNewName("");
      setError(null);
    } catch (err) {
      setError(codeOf(err) === "conflict" ? "已存在同名歌单" : "创建歌单失败");
    }
  }

  async function renamePlaylist() {
    const trimmed = renameValue.trim();
    if (!trimmed) {
      setError("名称不能为空");
      return;
    }
    const graphemes = countGraphemes(trimmed);
    if (graphemes > 40) {
      setError(`名称不能超过 40 个字符（当前 ${graphemes} 个）`);
      return;
    }
    try {
      await bridge.call("rename_playlist", { id: playlistId, name: trimmed });
      setName(trimmed);
      setRenameValue("");
      setRenaming(false);
      setError(null);
    } catch (err) {
      setError(codeOf(err) === "conflict" ? "已存在同名歌单" : "重命名失败");
    }
  }

  function beginRename() {
    setRenameValue(name);
    setRenaming(true);
    setError(null);
  }

  async function deletePlaylist() {
    setConfirmDelete(false);
    try {
      await bridge.call("delete_playlist", { id: playlistId });
      setError(null);
      onDeleted?.();
    } catch {
      setError("删除歌单失败");
    }
  }

  async function removeMember(song: SongView) {
    await bridge.call("remove_playlist_song", { playlist: playlistId, song: song.id });
    loadMembers();
  }

  return (
    <div className="workspace" data-testid="playlist-view">
      <div className="workspace-toolbar">
        <h2 className="workspace-title">歌单：{name || "…"}</h2>
        <div className="toolbar-actions">
          {renaming ? (
            <>
              <input
                aria-label="歌单新名称"
                className="search-input"
                placeholder="歌单新名称"
                value={renameValue}
                onChange={(e) => setRenameValue(e.target.value)}
              />
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => void renamePlaylist()}
              >
                保存
              </button>
              <button type="button" className="btn" onClick={() => setRenaming(false)}>
                取消
              </button>
            </>
          ) : (
            <>
              <button type="button" className="btn" onClick={beginRename}>
                重命名
              </button>
              {confirmDelete ? (
                <>
                  <span className="danger-text">删除歌单？</span>
                  <button
                    type="button"
                    className="btn btn-danger"
                    onClick={() => void deletePlaylist()}
                  >
                    确认删除
                  </button>
                  <button type="button" className="btn" onClick={() => setConfirmDelete(false)}>
                    取消
                  </button>
                </>
              ) : (
                <button type="button" className="btn" onClick={() => setConfirmDelete(true)}>
                  删除歌单
                </button>
              )}
            </>
          )}
        </div>
      </div>

      <div className="playlist-create">
        <input
          aria-label="新歌单名称"
          className="search-input"
          placeholder="新歌单名称"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
        />
        <button type="button" className="btn btn-primary" onClick={() => void createPlaylist()}>
          创建歌单
        </button>
        {error ? (
          <p className="workspace-error" role="alert">
            {error}
          </p>
        ) : null}
      </div>

      <p className="playlist-members-count">
        共 {members.length} 首（{members.filter((s) => s.availability !== "available").length}{" "}
        首不可用）
      </p>

      <SongList
        songs={members}
        search=""
        loading={false}
        isLast
        readOnly={false}
        currentSongId={snapshot.currentSongId}
        onLoadMore={() => {}}
        onClearSearch={() => {}}
        onPlay={() => {}}
        onFavorite={() => {}}
        onOpenMenu={setMenuFor}
      />

      {menuFor ? (
        <SongMenu
          song={menuFor}
          root={activeRoot()}
          readOnly={false}
          onClose={() => setMenuFor(null)}
          onPlay={() => setMenuFor(null)}
          onFavorite={() => setMenuFor(null)}
          onRefresh={loadMembers}
          extraActions={
            <button type="button" className="btn" onClick={() => void removeMember(menuFor)}>
              从歌单移除
            </button>
          }
        />
      ) : null}
    </div>
  );
}

/** Count user-perceived characters (grapheme clusters) without a dependency. */
export function countGraphemes(value: string): number {
  // Intl.Segmenter gives grapheme clusters on all modern engines; fall back to
  // Array.from (code points) where unavailable.
  if (typeof Intl !== "undefined" && "Segmenter" in Intl) {
    const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
    return Array.from(segmenter.segment(value)).length;
  }
  return Array.from(value).length;
}

function activeRoot(): string {
  // The desktop resolves the active root; a placeholder empty string is valid
  // for the command contract (the backend uses its own active root).
  return "";
}

function codeOf(err: unknown): string {
  if (err instanceof Error && "code" in err) {
    const code = (err as unknown as { code?: string }).code;
    return code ?? "";
  }
  return "";
}
