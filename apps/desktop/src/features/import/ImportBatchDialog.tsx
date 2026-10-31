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
import { libraryPersistenceMessage } from "../../bridge/libraryPersistence";

export interface ImportFailureDialogProps {
  readonly results: readonly ImportResultDto[];
  readonly onClose: () => void;
  readonly hasPending?: boolean;
  readonly onRecover?: (operationId: string) => void;
  readonly recoveringOperation?: string;
  readonly recoveryError?: string;
}

function ResultLine({
  result,
  onRecover,
  recoveringOperation,
}: {
  result: ImportResultDto;
  onRecover?: (operationId: string) => void;
  recoveringOperation?: string;
}) {
  if (result.kind === "savedPending") {
    return (
      <li data-testid="import-line">
        <p>已保存：{result.relativePath}</p>
        <p>
          {libraryPersistenceMessage(result.code) ?? "已保存到音乐库，等待更新。无需重新导入。"}
        </p>
        {result.lyrics.kind === "failed" ? (
          <p role="alert">音频已保存；歌词未导入：{result.lyrics.message}</p>
        ) : null}
        <small>恢复操作：{result.operationId}</small>
        {onRecover ? (
          <button
            type="button"
            className="btn"
            disabled={recoveringOperation !== undefined}
            onClick={() => onRecover(result.operationId)}
          >
            {recoveringOperation === result.operationId ? "正在恢复…" : "恢复此项"}
          </button>
        ) : null}
      </li>
    );
  }
  if (result.kind === "imported") {
    return (
      <li className="is-ok" data-testid="import-line">
        <p>
          已导入：{result.relativePath}
          {result.renamed ? "（已重名编号）" : ""}
        </p>
        {result.lyrics.kind === "failed" ? (
          <p role="alert">音频已导入；歌词未导入：{result.lyrics.message}</p>
        ) : null}
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
        音乐库不可用，未开始导入。
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

export function ImportFailureDialog({
  results,
  onClose,
  hasPending = false,
  onRecover,
  recoveringOperation,
  recoveryError,
}: ImportFailureDialogProps) {
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
        <h2 id="import-title">
          {hasPending ||
          results.some(
            (result) =>
              (result.kind === "imported" || result.kind === "savedPending") &&
              result.lyrics.kind === "failed",
          )
            ? "导入结果"
            : "导入失败"}
        </h2>
        <p className="dialog-summary">
          {hasPending
            ? "已保存的音乐等待更新，无需重新导入。歌词导入状态和未保存项目的失败原因会单独显示。"
            : "以下项目需要处理；已完成的导入不会受影响。"}
        </p>
        <ul className="dialog-results">
          {results.map((result, index) => (
            <ResultLine
              key={index}
              result={result}
              onRecover={onRecover}
              recoveringOperation={recoveringOperation}
            />
          ))}
        </ul>
        {recoveryError ? <p role="alert">{recoveryError}</p> : null}
        <div className="confirmation-actions">
          <button type="button" className="btn" onClick={onClose} data-testid="import-close">
            关闭
          </button>
        </div>
      </div>
    </section>
  );
}
