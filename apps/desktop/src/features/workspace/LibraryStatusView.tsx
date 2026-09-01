/**
 * Library unavailable / read-only status (task 10.3 / 10.7).
 *
 * When a root is configured but currently unreachable, or is read-only, the
 * workspace shows a clear status while preserving any already-present content.
 * It never fakes a success; a retry re-reads status / triggers a re-scan.
 */

import { bridge } from "../../bridge";
import type { LibraryStatus } from "./useLibraryStatus";

export function LibraryStatusView({ status }: { status: LibraryStatus }) {
  const reason = status.unavailable
    ? "资料库暂时不可用"
    : status.readOnly
      ? "资料库为只读（可浏览和播放，但导入与删除已禁用）"
      : "资料库尚未就绪";

  async function retry() {
    // A retry re-reads status; if a root is present we also offer a scan.
    await bridge.call("library_status");
    if (status.activeRoot) {
      await bridge.call("start_scan", { root: status.activeRoot });
    }
  }

  return (
    <div className="workspace-empty" data-testid="library-status" role="main">
      <h1 className="workspace-title">资料库状态</h1>
      <p className="workspace-hint">{reason}</p>
      <button type="button" className="btn" onClick={() => void retry()}>
        重新连接
      </button>
    </div>
  );
}
