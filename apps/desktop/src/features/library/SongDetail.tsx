import { useRef } from "react";

import type { SongView } from "../../ipc/ipc-types.generated";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { useOptionalCollectionNavigation } from "./collectionNavigationContext";

/**
 * Read-only song detail (task 10.8): relative path only, never absolute.
 *
 * Issue #118: the 艺人 / 专辑 rows are the same navigation links the list rows
 * carry, but the detail is a `Picker`-tier overlay above the 歌曲操作 menu, so a
 * link has to dismiss both layers before the new view appears. `onNavigate`
 * is the host's half of that — `SongMenu` closes the dialog and itself, then
 * this component asks the shell to switch views.
 */
export function SongDetail({
  song,
  onClose,
  onReveal,
  onNavigate,
}: {
  song: SongView;
  onClose: () => void;
  onReveal: () => void;
  /** Dismisses the detail and whatever opened it, before navigating. */
  onNavigate?: () => void;
}) {
  const detailRef = useRef<HTMLDivElement>(null);
  // A `Picker`-tier dialog above the menu: Escape closes it before the menu and
  // focus is trapped + restored.
  useOverlay({ tier: OverlayTier.Picker, onClose, containerRef: detailRef });
  useFocusTrap(detailRef);

  const navigation = useOptionalCollectionNavigation();
  const artist = song.artist ?? "未知艺人";
  const artistKey = song.artistKey;
  const albumKey = song.albumKey;

  const openArtist = () => {
    if (!artistKey) return;
    onNavigate?.();
    navigation?.openArtist(artistKey);
  };
  const openAlbum = () => {
    if (!albumKey) return;
    onNavigate?.();
    navigation?.openAlbum(albumKey);
  };

  return (
    <section
      className="confirmation-dialog"
      role="dialog"
      aria-modal="true"
      aria-label="歌曲详情"
      ref={detailRef}
    >
      <div className="confirmation-panel">
        <h2>{song.title ?? "未命名歌曲"}</h2>
        <dl className="detail-grid">
          <dt>艺人</dt>
          <dd>
            {artistKey && navigation ? (
              <button
                type="button"
                className="detail-link"
                aria-label={`打开歌手 ${artist}`}
                onClick={openArtist}
              >
                {artist}
              </button>
            ) : (
              artist
            )}
          </dd>
          <dt>专辑</dt>
          <dd>
            {albumKey && song.album && navigation ? (
              <button
                type="button"
                className="detail-link"
                aria-label={`打开专辑 ${song.album}`}
                onClick={openAlbum}
              >
                {song.album}
              </button>
            ) : (
              (song.album ?? "—")
            )}
          </dd>
          <dt>音乐库相对路径</dt>
          <dd data-testid="detail-relative-path">{song.relativePath}</dd>
          <dt>播放次数</dt>
          <dd>{song.playCount}</dd>
        </dl>
        <div className="confirmation-actions">
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
          <button type="button" className="btn btn-primary" onClick={onReveal}>
            打开本地目录
          </button>
        </div>
      </div>
    </section>
  );
}
