/**
 * 歌单命名对话框 (task 10.9) — the prototype's `.playlist-name-dialog`.
 *
 * The prototype reuses one dialog for both 新建歌单 and 编辑歌单: the title and
 * the submit label change, the field and the error line stay the same. That is
 * exactly this component — `mode="create"` calls `create_playlist`,
 * `mode="edit"` calls `rename_playlist`.
 *
 * Validation is the union of the prototype's own rules and the release spec
 * (task 10.9): 空名称 / 名称含竖线 / 超过 40 个字符 / 同名冲突, reported on the
 * prototype's `.playlist-name-error` line. The prototype's `maxlength="40"` is
 * deliberately not applied — it counts UTF-16 units, while the spec counts
 * grapheme clusters, so the explicit rule is what the user sees.
 */

import { useMemo, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { Icon } from "../../app/Icon";
import { coverClass } from "../library/coverPalette";

export interface PlaylistNameDialogProps {
  readonly mode: "create" | "edit";
  /** Required for `mode="edit"` — the playlist being renamed. */
  readonly playlistId?: string;
  readonly initialName?: string;
  /** Current effective cover, used as the edit preview. */
  readonly initialCoverKey?: string;
  readonly hasCustomCover?: boolean;
  /** Active library identity. Required for creates; never infer or fabricate it. */
  readonly root?: string;
  /** Names already in use; the current playlist's own name is excluded. */
  readonly existingNames: readonly string[];
  readonly onClose: () => void;
  /** Called with the accepted (trimmed) name after the mutation succeeds. */
  readonly onDone: (name: string, createdId?: string) => void;
}

export function PlaylistNameDialog({
  mode,
  playlistId,
  initialName = "",
  initialCoverKey,
  hasCustomCover = false,
  root,
  existingNames,
  onClose,
  onDone,
}: PlaylistNameDialogProps) {
  const [value, setValue] = useState(initialName);
  const [touched, setTouched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // `undefined` = untouched; `null` = explicitly restore automatic artwork.
  const [coverChange, setCoverChange] = useState<number[] | null | undefined>(undefined);
  const [coverMime, setCoverMime] = useState<string | null>(null);
  const [coverPreview, setCoverPreview] = useState<string | undefined>(initialCoverKey);
  const dialogRef = useRef<HTMLElement>(null);
  const coverInputRef = useRef<HTMLInputElement>(null);
  // The prototype puts this dialog in the `BlockingDialog` position of its own
  // stack (it is above the picker that may open it).
  useOverlay({ tier: OverlayTier.BlockingDialog, onClose, containerRef: dialogRef });
  useFocusTrap(dialogRef);

  const others = useMemo(
    () => existingNames.filter((name) => name !== initialName),
    [existingNames, initialName],
  );

  /** The prototype's `validatePlaylistName`, plus the 40-grapheme rule. */
  function validate(raw: string): string {
    const name = raw.trim();
    if (!name) return "请输入歌单名称。";
    if (name.includes("|")) return "歌单名称不能包含竖线。";
    const graphemes = countGraphemes(name);
    if (graphemes > 40) return `名称不能超过 40 个字符（当前 ${graphemes} 个）`;
    if (others.includes(name)) return "已存在同名歌单，请换一个名称。";
    return "";
  }

  const visible = touched || error !== null ? (error ?? validate(value)) : "";
  const creating = mode === "create";

  async function submit() {
    const message = validate(value);
    setTouched(true);
    if (message) {
      setError(message);
      return;
    }
    if (busy) return;
    if (creating && !root?.trim()) {
      setError("当前资料库不可用，请恢复后重试");
      return;
    }
    setBusy(true);
    const name = value.trim();
    try {
      if (creating) {
        const createdId = await bridge.call("create_playlist", { root, name });
        if (typeof createdId === "string") onDone(name, createdId);
        else onDone(name);
      } else {
        await bridge.call("rename_playlist", { id: playlistId, name });
        if (coverChange !== undefined) {
          await bridge.call("set_playlist_cover", {
            id: playlistId,
            bytes: coverChange,
            mime: coverChange ? coverMime : null,
          });
        }
        onDone(name);
      }
      onClose();
    } catch (err) {
      // A duplicate name is rejected server-side too (task 6.6); never invent a
      // success for it.
      setError(
        codeOf(err) === "conflict" ? "已存在同名歌单，请换一个名称。" : "保存歌单失败，请重试",
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      className="playlist-name-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="playlist-name-title"
      ref={dialogRef}
    >
      <form
        className="playlist-name-panel"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <h2 id="playlist-name-title">{creating ? "新建歌单" : "编辑歌单"}</h2>
        <div className="playlist-name-field">
          <input
            id="playlist-name-input"
            type="text"
            autoComplete="off"
            placeholder="输入歌单名称"
            aria-label={creating ? "新歌单名称" : "歌单新名称"}
            aria-invalid={visible ? true : undefined}
            aria-describedby="playlist-name-error"
            value={value}
            autoFocus
            onChange={(event) => {
              setValue(event.target.value);
              if (touched) setError(validate(event.target.value) || null);
            }}
            onBlur={() => {
              setTouched(true);
              setError(validate(value) || null);
            }}
          />
          <p
            className="playlist-name-error"
            id="playlist-name-error"
            role="alert"
            hidden={!visible}
          >
            {visible}
          </p>
        </div>
        {!creating ? (
          <div className="playlist-cover-field">
            <span>歌单封面</span>
            <button
              type="button"
              className="playlist-cover-preview"
              aria-label="选择歌单封面图片"
              onClick={() => coverInputRef.current?.click()}
            >
              <span className={`cover ${coverClass(playlistId ?? "playlist")}${coverPreview ? " has-image" : ""}`}>
                {coverPreview ? <img src={coverPreview.startsWith("blob:") ? coverPreview : bridge.assetUrl(coverPreview)} alt="当前歌单封面" /> : null}
              </span>
              <span className="playlist-cover-hint-overlay"><Icon name="edit" />更换图片</span>
            </button>
            <input
              ref={coverInputRef}
              type="file"
              accept="image/png,image/jpeg,image/webp"
              hidden
              onChange={(event) => {
                const file = event.currentTarget.files?.[0];
                event.currentTarget.value = "";
                if (!file) return;
                if (file.size > 5 * 1024 * 1024) { setError("封面图片不能超过 5 MB。"); return; }
                const reader = new FileReader();
                reader.onload = () => {
                  const bytes = new Uint8Array(reader.result as ArrayBuffer);
                  setCoverChange(Array.from(bytes));
                  setCoverMime(file.type);
                  setCoverPreview(URL.createObjectURL(file));
                  setError(null);
                };
                reader.onerror = () => setError("读取封面图片失败，请重试。");
                reader.readAsArrayBuffer(file);
              }}
            />
            {coverChange !== null && (coverChange !== undefined || hasCustomCover) ? (
              <button type="button" className="btn" onClick={() => { setCoverChange(null); setCoverMime(null); setCoverPreview(undefined); }}>
                恢复自动封面
              </button>
            ) : null}
          </div>
        ) : null}
        <div className="playlist-name-actions">
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            {creating ? "创建歌单" : "保存"}
          </button>
        </div>
      </form>
    </section>
  );
}

/** The 新建歌单 entry — the sidebar `+` and the picker's 新建歌单 both use this. */
export function PlaylistCreateDialog({
  existingNames,
  root,
  onClose,
  onCreated,
}: {
  readonly existingNames: readonly string[];
  readonly root?: string;
  readonly onClose: () => void;
  readonly onCreated?: (name: string, createdId?: string) => void;
}) {
  return (
    <PlaylistNameDialog
      mode="create"
      root={root}
      existingNames={existingNames}
      onClose={onClose}
      onDone={(name, createdId) => onCreated?.(name, createdId)}
    />
  );
}

/** Count user-perceived characters (grapheme clusters) without a dependency. */
export function countGraphemes(value: string): number {
  // Intl.Segmenter gives grapheme clusters on all modern engines; fall back to
  // Array.from (code points) where unavailable.
  if (typeof Intl !== "undefined" && "Segmenter" in Intl) {
    const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
    return Array.from(segmenter.segment(value)).length;
  }
  return Array.from(value).length;
}

function codeOf(err: unknown): string {
  if (err instanceof Error && "code" in err) {
    return (err as unknown as { code?: string }).code ?? "";
  }
  return "";
}
