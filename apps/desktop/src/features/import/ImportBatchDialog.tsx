/**
 * Multi-select import (task 10.10).
 *
 * Triggers the Rust file dialog (choose_and_import_files → per-input results)
 * and renders a per-file report: imported / duplicate / unsupported /
 * library-unavailable / failed, with reasons. A mixed batch never asks the user
 * to redo the successes; a failed item is retryable without touching the
 * already-imported ones (a fresh dialog re-plans and dedups by BLAKE3).
 *
 * Surface: the prototype never draws a batch report (its import only toasts),
 * so this is built entirely from prototype primitives — the
 * `.confirmation-dialog` / `.confirmation-panel` modal anatomy plus the
 * `.dialog-results` list. No bespoke dialog chrome.
 */

import { useRef, useState } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import type { ImportBatchDto, ImportResultDto } from "../../ipc/ipc-types.generated";

export interface ImportBatchDialogProps {
  readonly onClose: () => void;
  readonly onDone: (batch: ImportBatchDto) => void;
}

function ResultLine({ result }: { result: ImportResultDto }) {
  if (result.kind === "imported") {
    return (
      <li className="is-ok" data-testid="import-line">
        已导入：{result.relativePath}
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
  if (result.kind === "unsupported") {
    return (
      <li className="is-fail" data-testid="import-line">
        不支持的文件类型。
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

export function ImportBatchDialog({ onClose, onDone }: ImportBatchDialogProps) {
  const [batch, setBatch] = useState<ImportBatchDto | null>(null);
  const [busy, setBusy] = useState(false);
  const dialogRef = useRef<HTMLElement>(null);
  // A `BlockingDialog`-tier modal: Escape closes it last on the single stack,
  // focus is trapped and restored on close.
  useOverlay({ tier: OverlayTier.BlockingDialog, onClose, containerRef: dialogRef });
  useFocusTrap(dialogRef);

  async function chooseAndImport() {
    setBusy(true);
    try {
      const result = await bridge.call("choose_and_import_files");
      if (result === null) {
        // Cancelled dialog: no success, no failure — a genuine no-op.
        onClose();
        return;
      }
      setBatch(result);
      onDone(result);
    } finally {
      setBusy(false);
    }
  }

  const count = (kind: ImportResultDto["kind"]) =>
    batch?.results.filter((r) => r.kind === kind).length ?? 0;

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
        <h2 id="import-title">导入歌曲</h2>
        {!batch ? (
          <p>选择要导入到资料库的音频文件（可多选）。</p>
        ) : (
          <>
            <p className="dialog-summary">
              成功 {count("imported")}，重复 {count("duplicate")}，不支持 {count("unsupported")}
              ，失败 {count("failed")}
              {count("libraryUnavailable") > 0 ? "，资料库不可用" : ""}
            </p>
            <ul className="dialog-results">
              {batch.results.map((r, i) => (
                <ResultLine key={i} result={r} />
              ))}
            </ul>
          </>
        )}
        <div className="confirmation-actions">
          <button type="button" className="btn" onClick={onClose} data-testid="import-close">
            关闭
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => void chooseAndImport()}
            disabled={busy}
          >
            {busy ? "导入中…" : batch ? "再次导入" : "选择文件并导入"}
          </button>
        </div>
      </div>
    </section>
  );
}
