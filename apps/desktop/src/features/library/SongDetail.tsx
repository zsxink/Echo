import { useRef } from "react";

import type { SongView } from "../../ipc/ipc-types.generated";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";

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
          <dd>{song.artist ?? "未知艺人"}</dd>
          <dt>专辑</dt>
          <dd>{song.album ?? "—"}</dd>
          <dt>资料库相对路径</dt>
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
