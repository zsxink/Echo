/**
 * Multi-select import (task 10.10).
 *
 * Triggers the Rust file dialog (choose_and_import_files → per-input results)
 * and renders a per-file report: imported / duplicate / skipped /
 * library-unavailable / failed, with reasons. A mixed batch never asks the user
 * to redo the successes; a failed item is retryable without touching the
 * already-imported ones (a fresh dialog re-plans and dedups by BLAKE3).
 *
 * Surface: the prototype never draws a batch report (its import only toasts),
 * so this is built entirely from prototype primitives — the
 * `.confirmation-dialog` / `.confirmation-panel` modal anatomy plus the
 * `.dialog-results` list. No bespoke dialog chrome.
 */

import { useRef } from "react";

import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import type { ImportResultDto } from "../../ipc/ipc-types.generated";

export interface ImportFailureDialogProps {
  readonly results: readonly ImportResultDto[];
  readonly onClose: () => void;
}

function ResultLine({ result }: { result: ImportResultDto }) {
  if (result.kind === "imported") {
    return (
      <li className="is-ok" data-testid="import-line">
        已导入：{result.relativePath}
        {result.renamed ? "（已重名编号）" : ""}
      </li>
    );
  }
  if (result.kind === "duplicate") {
    return (
      <li className="is-dup" data-testid="import-line">
        内容重复（已有歌曲 {result.existingSongId}），未重复导入。
      </li>
    );
  }
  if (result.kind === "skipped") {
    return (
      <li className="is-ok" data-testid="import-line">
        已跳过：非音频文件，无需处理。
      </li>
    );
  }
  if (result.kind === "libraryUnavailable") {
    return (
      <li className="is-fail" data-testid="import-line">
        资料库不可用，未开始导入。
      </li>
    );
  }
  if (result.kind === "failed") {
    return (
      <li className="is-fail" data-testid="import-line">
        失败：{result.message}
      </li>
    );
  }
  return null;
}

export function ImportFailureDialog({ results, onClose }: ImportFailureDialogProps) {
  const dialogRef = useRef<HTMLElement>(null);
  // A `BlockingDialog`-tier modal: Escape closes it last on the single stack,
  // focus is trapped and restored on close.
  useOverlay({ tier: OverlayTier.BlockingDialog, onClose, containerRef: dialogRef });
  useFocusTrap(dialogRef);

  return (
    <section
      className="confirmation-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="import-title"
      data-testid="import-dialog"
      ref={dialogRef}
    >
      <div className="confirmation-panel">
        <h2 id="import-title">导入失败</h2>
        <p className="dialog-summary">以下项目需要处理；已完成的导入不会受影响。</p>
        <ul className="dialog-results">
          {results.map((result, index) => (
            <ResultLine key={index} result={result} />
          ))}
        </ul>
        <div className="confirmation-actions">
          <button type="button" className="btn" onClick={onClose} data-testid="import-close">
            关闭
          </button>
        </div>
      </div>
    </section>
  );
}
