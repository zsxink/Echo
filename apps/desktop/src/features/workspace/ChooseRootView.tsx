/**
 * First-launch / unconfigured workspace (task 10.3).
 *
 * Guides the user to choose a new or existing library root. A cancel keeps this
 * view (it never fabricates a scan success); a confirmed root activates and the
 * shell re-renders into the library. Rejected roots surface a reason and a
 * re-choose entrance without overwriting any prior config.
 *
 * The prototype always ships a configured library, so this page is the one
 * surface it never draws; it is built from the prototype's own primitives
 * (`.eyebrow` + display heading + body copy + `.btn.btn-primary`) so it reads as
 * the same application.
 */

import { useState } from "react";

import { bridge, BridgeError } from "../../bridge";
import { invalidateCovers } from "../../app/coverArt";
import type { LibraryRootStatusDto } from "../../ipc/ipc-types.generated";

export function ChooseRootView({ onActivated }: { onActivated: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function chooseRoot() {
    setBusy(true);
    setError(null);
    try {
      const result = (await bridge.call("choose_library_root")) as LibraryRootStatusDto | null;
      if (result === null) {
        // Cancelled: stay on the initialize view — never a scan/success claim.
        return;
      }
      // Refresh the shell explicitly: activation need not emit a status event.
      // A new root may reuse the same SongIds for different files, so cached
      // artwork must not survive the switch.
      invalidateCovers();
      await onActivated();
    } catch (err) {
      const message = err instanceof BridgeError ? err.message : "选择资料库时出错";
      setError(message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="workspace-empty" data-testid="choose-root" role="main">
      <p className="eyebrow">本地资料库</p>
      <h1>选择本地资料库</h1>
      <p>
        选择一个文件夹开始使用，也可以打开之前使用的 Echo
        资料库。文件夹可以为空；已有歌曲会自动扫描。
      </p>
      <button
        type="button"
        className="btn btn-primary"
        onClick={() => void chooseRoot()}
        disabled={busy}
      >
        {busy ? "正在处理…" : "选择资料库目录"}
      </button>
      {error ? (
        <p className="status-error" role="alert">
          {error}（可选择其他目录重试）
        </p>
      ) : null}
    </div>
  );
}
