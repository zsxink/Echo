/**
 * Artist and album directory for the local library.
 *
 * Core owns grouping and stable identities; this component only retains the
 * current directory/detail request so a late result cannot replace a newer
 * view, search, or active-root selection.
 */

import { useEffect, useMemo, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { useCoverKeys } from "../../app/coverArt";
import { Icon } from "../../app/Icon";
import { Topbar } from "../../app/shell";
import { notify } from "../../app/toast";
import type { CatalogCollectionView, SongView } from "../../ipc/ipc-types.generated";
import { coverClass } from "./coverClass";
import { subscribeLibraryInvalidations } from "./libraryInvalidation";
import { SelectionModeButton } from "./BatchSongActions";
import { useSongSelection } from "./useSongSelection";

export interface CollectionDirectoryProps {
  readonly kind: "artist" | "album";
  readonly root: string;
}

export function CollectionDirectory({ kind, root }: CollectionDirectoryProps) {
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<readonly CatalogCollectionView[]>([]);
  const [selected, setSelected] = useState<CatalogCollectionView | null>(null);
  const [songs, setSongs] = useState<readonly SongView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  const request = useRef(0);
  const title = kind === "artist" ? "歌手" : "专辑";
  const songCovers = useCoverKeys(songs.map((song) => song.id));
  const [selectionMode, setSelectionMode] = useState(false);
  const selectionKey = useMemo(
    () => [kind, root, selected?.artistKey ?? "", selected?.albumKey ?? "", search].join("|"),
    [kind, root, search, selected],
  );
  const selection = useSongSelection(selectionKey);
  const allSongsSelected =
    songs.length > 0 && songs.every((song) => selection.selectedIds.has(song.id));

  useEffect(() => {
    setSelected(null);
    setSongs([]);
    setSelectionMode(false);
  }, [kind, root]);

  useEffect(() => {
    setSelectionMode(false);
    selection.clear();
  }, [selection.clear, selected]);

  useEffect(() => {
    const id = ++request.current;
    setLoading(true);
    setError(null);
    void bridge
      .call("catalog_collections", { kind, search })
      .then((result) => {
        if (request.current === id) setEntries(result);
      })
      .catch(() => {
        if (request.current === id) setError("加载目录失败，请重试");
      })
      .finally(() => {
        if (request.current === id) setLoading(false);
      });
  }, [kind, root, search, refreshEpoch]);

  useEffect(() => {
    if (!selected) return;
    const id = ++request.current;
    setLoading(true);
    setError(null);
    void bridge
      .call("catalog_collection_songs", {
        kind,
        artistKey: selected.artistKey,
        albumKey: selected.albumKey ?? null,
        search,
      })
      .then((result) => {
        if (request.current === id) setSongs(result);
      })
      .catch(() => {
        if (request.current === id) setError("加载歌曲失败，请重试");
      })
      .finally(() => {
        if (request.current === id) setLoading(false);
      });
  }, [kind, root, search, selected]);

  useEffect(() => subscribeLibraryInvalidations(() => setRefreshEpoch((epoch) => epoch + 1)), []);

  function play(song: SongView) {
    const collectionView = kind === "artist" ? "artist" : "album";
    bridge.fireAndForget("play_library_context", {
      view: collectionView,
      query: search,
      sort: "addedAt:desc",
      selectedSong: song.id,
      artistKey: selected?.artistKey,
      albumKey: selected?.albumKey,
    });
  }

  function toggleFavorite(song: SongView) {
    void bridge
      .call("set_favorite", { songId: song.id, favorite: !song.favorite })
      .then((committed) => {
        setSongs((current) => current.map((item) => (item.id === song.id ? committed : item)));
      })
      .catch(() => notify({ message: "收藏失败，请重试", error: true }));
  }

  const detailTitle = selected ? selected.name : title;
  const detailSub = selected
    ? kind === "artist"
      ? `${selected.songCount} 首歌曲`
      : `${selected.artist} · ${selected.songCount} 首歌曲`
    : null;

  return (
    <>
      <Topbar title={detailTitle}>
        <label className="search" data-testid="collection-search-field">
          <Icon name="search" />
          <input
            type="search"
            placeholder={kind === "artist" ? "搜索歌手" : "搜索专辑或歌手"}
            aria-label={kind === "artist" ? "搜索歌手" : "搜索专辑"}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
      </Topbar>

      <main className="content" data-testid={`${kind}-directory`}>
        <div className="library-view">
          <div className="library-head">
            <div className="list-context">
              {selected ? (
                <button
                  className="collection-back is-visible"
                  type="button"
                  aria-label={`返回${title}`}
                  onClick={() => setSelected(null)}
                >
                  <Icon name="chevronLeft" />
                </button>
              ) : null}
              <div>
                <h1>{detailTitle}</h1>
                {detailSub ? <span className="library-total">{detailSub}</span> : null}
              </div>
            </div>
            {selected ? (
              <div className="library-tools">
                <SelectionModeButton
                  active={selectionMode}
                  onToggle={() => {
                    if (selectionMode) {
                      selection.clear();
                      setSelectionMode(false);
                    } else {
                      setSelectionMode(true);
                    }
                  }}
                />
              </div>
            ) : null}
          </div>

          {selected ? (
            <section className="collection-song-list" aria-label={`${selected.name}的歌曲`}>
              {selectionMode ? (
                <div className="collection-song-head">
                  <button
                    type="button"
                    className={`list-select-all${allSongsSelected ? " active" : ""}`}
                    aria-label={allSongsSelected ? "取消全选当前歌曲" : "全选当前歌曲"}
                    aria-pressed={allSongsSelected}
                    disabled={songs.length === 0}
                    onClick={() => selection.toggleAllLoaded(songs.map((song) => song.id))}
                  >
                    {allSongsSelected ? <Icon name="check" /> : null}
                  </button>
                  <span>
                    {selection.selectedCount > 0
                      ? `已选 ${selection.selectedCount} 首`
                      : "选择歌曲"}
                  </span>
                </div>
              ) : null}
              {songs.map((song, index) => (
                <div
                  className={`collection-song${selectionMode ? " selection-mode" : ""}${selection.selectedIds.has(song.id) ? " selected" : ""}`}
                  key={song.id}
                >
                  {selectionMode ? (
                    <button
                      type="button"
                      className={`row-select${selection.selectedIds.has(song.id) ? " active" : ""}`}
                      aria-label={
                        selection.selectedIds.has(song.id)
                          ? `取消选择${song.title ?? "歌曲"}`
                          : `选择${song.title ?? "歌曲"}`
                      }
                      aria-pressed={selection.selectedIds.has(song.id)}
                      onClick={() => selection.toggle(song.id)}
                    >
                      {selection.selectedIds.has(song.id) ? <Icon name="check" /> : null}
                    </button>
                  ) : null}
                  <button type="button" className="collection-song-main" onClick={() => play(song)}>
                    <span className="track-number">{String(index + 1).padStart(2, "0")}</span>
                    <span
                      className={`cover ${coverClass(song.id)}${songCovers.get(song.id) ? " has-image" : ""}`}
                      aria-hidden="true"
                    >
                      {songCovers.get(song.id) ? (
                        <img src={bridge.assetUrl(songCovers.get(song.id)!)} alt="" />
                      ) : null}
                    </span>
                    <span className="collection-song-copy">
                      <strong>{song.title ?? "未命名歌曲"}</strong>
                      <span>{song.artist ?? "未知艺人"}</span>
                    </span>
                    <span className="track-time">{formatDuration(song.durationS)}</span>
                  </button>
                  <button
                    type="button"
                    className={`row-action favorite${song.favorite ? " active" : ""}`}
                    aria-label={
                      song.favorite
                        ? `取消喜欢${song.title ?? "歌曲"}`
                        : `喜欢${song.title ?? "歌曲"}`
                    }
                    aria-pressed={song.favorite}
                    onClick={() => toggleFavorite(song)}
                  >
                    <Icon name="heart" />
                  </button>
                </div>
              ))}
            </section>
          ) : (
            <section className="collection-directory" aria-busy={loading}>
              <div className="collection-grid">
                {entries.map((entry) => (
                  <article
                    className="collection-card"
                    key={`${entry.artistKey}:${entry.albumKey ?? ""}`}
                  >
                    <button
                      type="button"
                      className="collection-open"
                      aria-label={`打开${title} ${entry.name}`}
                      onClick={() => setSelected(entry)}
                    >
                      <span
                        className={`collection-art cover ${coverClass(entry.artistKey)}${entry.coverKey ? " has-image" : ""}`}
                        aria-hidden="true"
                      >
                        {entry.coverKey ? (
                          <img src={bridge.assetUrl(entry.coverKey)} alt="" />
                        ) : null}
                      </span>
                      <span className="collection-copy">
                        <strong>{entry.name}</strong>
                        <span>
                          {kind === "artist"
                            ? `${entry.songCount} 首歌曲`
                            : `${entry.artist} · ${entry.songCount} 首歌曲`}
                        </span>
                      </span>
                      <span className="collection-chevron" aria-hidden="true">
                        <Icon name="chevronRight" />
                      </span>
                    </button>
                  </article>
                ))}
              </div>
              {!loading && entries.length === 0 ? (
                <div className="collection-empty show">
                  <h2>没有找到匹配的内容</h2>
                  <p>试试其他{title}名称。</p>
                </div>
              ) : null}
            </section>
          )}
          {error ? <p className="collection-error">{error}</p> : null}
        </div>
      </main>
    </>
  );
}

function formatDuration(seconds: number | undefined): string {
  if (seconds === undefined) return "--:--";
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
