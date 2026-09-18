import { useState } from "react";

import { bridge } from "../../bridge";

/**
 * "导入到资料库" inside the immersive surface when the current entry is a
 * session-only temporary item. Shows inline status after the import completes
 * and is disabled while the request is in flight.
 */
export function ImmersiveImport() {
  const [importing, setImporting] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  async function doImport() {
    if (importing) return;
    setImporting(true);
    setResult(null);
    try {
      const res = await bridge.call("import_current_temporary_file");
      setResult(importResultText(res.kind));
    } catch {
      setResult("导入失败");
    } finally {
      setImporting(false);
    }
  }

  return (
    <div className="now-playing-import" data-testid="immersive-import">
      <span className="player-temporary-tag" aria-label="临时播放项">
        临时
      </span>
      {result ? (
        <span className="now-playing-import-result" data-testid="import-result">
          {result}
        </span>
      ) : (
        <button
          type="button"
          className="btn"
          disabled={importing}
          aria-label="导入到资料库"
          onClick={() => void doImport()}
        >
          {importing ? "导入中…" : "导入到资料库"}
        </button>
      )}
    </div>
  );
}

function importResultText(kind: string): string {
  switch (kind) {
    case "imported":
      return "已导入到资料库";
    case "duplicate":
      return "资料库已有相同歌曲";
    case "skipped":
      return "不支持的格式，未导入";
    case "failed":
      return "导入失败";
    default:
      return "导入完成";
  }
}
