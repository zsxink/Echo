import { useRef } from "react";

import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import type { ScanIssueDto } from "../../ipc/ipc-types.generated";

export function ScanReportDialog({
  issues,
  onClose,
}: {
  issues: readonly ScanIssueDto[];
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLElement>(null);
  useOverlay({ tier: OverlayTier.BlockingDialog, onClose, containerRef: dialogRef });
  useFocusTrap(dialogRef);

  return (
    <section
      className="confirmation-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="scan-report-title"
      data-testid="scan-report-dialog"
      ref={dialogRef}
    >
      <div className="confirmation-panel">
        <h2 id="scan-report-title">扫描结果</h2>
        <p className="dialog-summary">{issues.length} 个文件需要查看。其他文件仍按扫描结果处理。</p>
        <ul className="dialog-results">
          {issues.map((issue, index) => (
            <li key={index} data-testid="scan-issue">
              <p>{issue.relativePath}</p>
              <p role="status">{issue.message}</p>
              <p>{issue.songCreated ? "歌曲记录已保留" : "未创建歌曲记录"}</p>
            </li>
          ))}
        </ul>
        <div className="confirmation-actions">
          <button type="button" className="btn" onClick={onClose} data-testid="scan-report-close">
            关闭
          </button>
        </div>
      </div>
    </section>
  );
}
