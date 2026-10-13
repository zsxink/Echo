import { useRef, useState } from "react";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";

export type EffectsDialogAction =
  | { readonly kind: "save" }
  | { readonly kind: "rename" | "delete"; readonly id: string; readonly name: string };

export function EffectsDialog({
  action,
  onClose,
  onSubmit,
}: {
  readonly action: EffectsDialogAction;
  readonly onClose: () => void;
  readonly onSubmit: (name: string) => Promise<void>;
}) {
  const container = useRef<HTMLElement>(null);
  const [name, setName] = useState(action.kind === "save" ? "" : action.name);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useOverlay({
    tier: OverlayTier.BlockingDialog,
    containerRef: container,
    onClose: () => {
      if (!busy) onClose();
    },
  });
  useFocusTrap(container);
  const deleting = action.kind === "delete";
  const title = deleting ? "删除曲线" : action.kind === "save" ? "保存曲线" : "重命名曲线";
  return (
    <section
      className="playlist-name-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="effects-dialog-title"
      ref={container}
    >
      <form
        className="playlist-name-panel"
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          if (!deleting && !name.trim()) {
            setError("请输入曲线名称。");
            return;
          }
          setBusy(true);
          setError(null);
          void onSubmit(name)
            .then(onClose)
            .catch((cause: unknown) => {
              const code = cause instanceof Error && "code" in cause ? String(cause.code) : "";
              setError(
                code === "validation"
                  ? "名称无效、已存在或已达到 50 条曲线，请检查后重试。"
                  : "保存失败，输入已保留。请检查磁盘空间和权限后重试。",
              );
            })
            .finally(() => setBusy(false));
        }}
      >
        <h2 id="effects-dialog-title">{title}</h2>
        {deleting ? (
          <p>删除「{action.name}」？当前曲线如已启用，将先关闭音效。</p>
        ) : (
          <div className="playlist-name-field">
            <label htmlFor="effects-name">曲线名称（1–40 个字符）</label>
            <input
              id="effects-name"
              type="text"
              autoComplete="off"
              value={name}
              onChange={(event) => setName(event.target.value)}
              aria-invalid={Boolean(error)}
              aria-describedby="effects-dialog-error"
            />
          </div>
        )}
        <p id="effects-dialog-error" role="status" className="playlist-name-error">
          {error}
        </p>
        <div className="playlist-name-actions">
          <button type="button" className="btn" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            {busy ? "正在保存…" : deleting ? "删除" : "保存"}
          </button>
        </div>
      </form>
    </section>
  );
}
