/**
 * Library unavailable / read-only status (task 10.3 / 10.7).
 *
 * When a root is configured but currently unreachable, or is read-only, the
 * workspace shows a clear status while preserving any already-present content.
 * It never fakes a success; a retry re-reads status / triggers a re-scan.
 *
 * The standalone prototype includes this state as `?preview=unavailable`; the
 * app wires its retry action to the current library root and scan coordinator.
 */

import { bridge } from "../../bridge";
import { invalidateCovers } from "../../app/coverArt";
import type { LibraryStatus } from "./useLibraryStatus";

export function LibraryStatusView({ status }: { status: LibraryStatus }) {
  const reason = status.unavailable
    ? "资料库暂时不可用"
    : status.readOnly
      ? "资料库为只读（可浏览和播放，但导入与删除已禁用）"
      : "资料库尚未就绪";

  async function retry() {
    // A retry re-reads status; if a root is present we also offer a scan. A
    // re-scan may re-parse embedded artwork under the same SongId, so the
    // cached keys must not outlive it.
    await bridge.call("library_status");
    if (status.activeRoot) {
      invalidateCovers();
      await bridge.call("start_scan", { root: status.activeRoot });
      // A rescan can make the previously remembered song unavailable. Apply
      // the same initial-song fallback after the scan has reconciled records.
      await bridge.call("restore_playback_session", {}).catch(() => undefined);
    }
  }

  return (
    <div className="workspace-empty" data-testid="library-status" role="main">
      <p className="eyebrow">资料库状态</p>
      <h1>{reason}</h1>
      <p>已保存的歌曲仍在磁盘上；重新连接后 Echo 会继续使用同一份资料库，不会新建或覆盖。</p>
      <button type="button" className="btn" onClick={() => void retry()}>
        重新连接
      </button>
    </div>
  );
}
