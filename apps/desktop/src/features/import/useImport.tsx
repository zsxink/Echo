/**
 * Shared multi-file import flow (playlist-search-locate-import 5.1).
 *
 * Import used to live inside `LibraryWorkspace` and be surfaced exclusively
 * from its topbar, which left 歌单/歌手/专辑 views without an entry. This hook
 * captures the whole flow — `choose_and_import_files` → classify → invalidate →
 * failure dialog — so the App shell can render one import control that works
 * under every library view. The library-refresh semantics stay injectable: the
 * shell owns `invalidateLibrary` + the playlist reload, and a committed import
 * asks the current view to re-read through those callbacks.
 */

import { useCallback, useState, type ReactNode } from "react";

import { bridge } from "../../bridge";
import type { ImportResultDto } from "../../ipc/ipc-types.generated";
import { classifyImportResults } from "./importFeedback";
import { ImportFailureDialog } from "./ImportBatchDialog";
import { notify } from "../../app/toast";
import { invalidateLibrary } from "../library";

export interface UseImportOptions {
  /** Called after a committed import so this view's list re-reads. */
  readonly onImportCommitted?: () => void;
}

export interface UseImport {
  readonly importing: boolean;
  readonly runImport: () => Promise<void>;
  readonly renderImportDialog: () => ReactNode;
}

/**
 * Own one `choose_and_import_files` flow: a busy flag, the happy-path
 * invalidation + toast, and a failure dialog for the parts that did not land.
 * Only failures render a dialog — non-failures are acknowledged by the toast.
 */
export function useImport({ onImportCommitted }: UseImportOptions = {}): UseImport {
  const [importing, setImporting] = useState(false);
  const [importFailures, setImportFailures] = useState<readonly ImportResultDto[] | null>(null);

  const runImport = useCallback(async () => {
    if (importing) return;
    setImporting(true);
    try {
      const batch = await bridge.call("choose_and_import_files");
      if (batch === null) return;
      const feedback = classifyImportResults(batch.results);
      if (feedback.nonFailures.length > 0) invalidateLibrary();
      onImportCommitted?.();
      if (feedback.nonFailures.length > 0) notify(feedback.summary);
      if (feedback.failures.length > 0) setImportFailures(feedback.failures);
    } catch (error) {
      const message = error instanceof Error ? error.message : "导入命令失败";
      setImportFailures([
        {
          kind: "failed",
          code: "import_command",
          message,
        },
      ]);
    } finally {
      setImporting(false);
    }
  }, [importing, onImportCommitted]);

  const renderImportDialog = useCallback(() => {
    if (!importFailures) return null;
    return <ImportFailureDialog results={importFailures} onClose={() => setImportFailures(null)} />;
  }, [importFailures]);

  return { importing, runImport, renderImportDialog };
}
