/**
 * First-launch / unconfigured workspace (task 10.3).
 *
 * Guides the user to choose a read-only library root. A cancel keeps this
 * view (it never fabricates a scan success); a confirmed root activates and
 * the shell re-renders into the library. Unavailable/rejected roots surface a
 * reason and a re-choose entrance without overwriting any prior config.
 */

import { useState } from "react";

import { bridge } from "../../bridge";
import type { LibraryRootStatusDto } from "../../ipc/ipc-types.generated";

export function ChooseRootView() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activated, setActivated] = useState(false);

  async function chooseRoot() {
    setBusy(true);
    setError(null);
    try {
      const result = (await bridge.call("choose_library_root")) as LibraryRootStatusDto | null;
      if (result === null) {
        // Cancelled: stay on the initialize view — never a scan/success claim.
        setBusy(false);
        return;
      }
      // A real activation: the shell re-reads library_status and re-renders.
      setActivated(true);
    } catch (err) {
      const message =
        err instanceof Error && err.name === "BridgeError"
          ? ((err as { code?: string; messageKey?: string }).messageKey ?? "选择资料库时出错")
          : "选择资料库时出错";
      setError(message);
      setBusy(false);
    }
  }

  return (
    <div className="workspace-empty" data-testid="choose-root" role="main">
      <h1 className="workspace-title">选择本地资料库</h1>
      <p className="workspace-hint">
        Echo 是本地优先播放器。请选择一个包含音乐文件的文件夹作为资料库根目录。
      </p>
      <button
        type="button"
        className="btn btn-primary"
        onClick={() => void chooseRoot()}
        disabled={busy || activated}
      >
        {busy ? "正在处理…" : "选择资料库目录"}
      </button>
      {activated ? <p className="workspace-ok">资料库已激活，正在载入…</p> : null}
      {error ? (
        <p className="workspace-error" role="alert">
          {error}（可选择其他目录重试）
        </p>
      ) : null}
    </div>
  );
}
