/**
 * 设置 (task 10.1 theme UI; close behavior surfaced from the desktop preference).
 *
 * Reproduces the prototype's `.settings-dialog`: a centred `.settings-panel`
 * with `.settings-dialog-head` (eyebrow + 设置 + close), a scrolling
 * `.settings-scroll`, and `.settings-section` blocks — 本地存储 / 主题 / 关于 Echo,
 * with the prototype's own copy.
 *
 * Two things the prototype's sample cannot show are added on the same primitives:
 *  - 音乐目录 prints the *real* active root the desktop reports, or "尚未选择音乐目录";
 *    it never prints a path Echo does not have.
 *  - 应用行为 carries the desktop's real 关闭主窗口时 preference. The prototype has
 *    no counterpart section, so it is built from `.settings-section` / `.setting-row`
 *    rather than inventing new surface.
 *
 * The favourite-heart red and other status semantics are not part of the theme
 * choice and are never overridden.
 */

import { useRef, useState } from "react";

import { bridge } from "../../bridge";
import { Icon } from "../../app/Icon";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import type { CloseBehavior } from "../../ipc/ipc-types.generated";
import { useLibraryStatus } from "../workspace/useLibraryStatus";
import { useTheme, type Theme } from "./useTheme";

/**
 * The prototype's three theme options. `swatch` is the prototype's own
 * `.theme-swatch.<name>` class, which carries the brand colour; `value` is the
 * persisted theme id the desktop stores (coral is the prototype's 珊瑚玫红).
 */
const THEME_OPTIONS: readonly {
  readonly value: Theme;
  readonly swatch: "wine" | "cobalt" | "green";
  readonly name: string;
  readonly hint: string;
}[] = [
  { value: "cobalt", swatch: "cobalt", name: "深钴蓝", hint: "冷静、专注" },
  { value: "coral", swatch: "wine", name: "珊瑚玫红", hint: "默认主题" },
  { value: "turquoise", swatch: "green", name: "松石绿", hint: "平衡、舒缓" },
];

const ABOUT: readonly { readonly title: string; readonly body: string }[] = [
  {
    title: "本地优先",
    body: "管理自己的音乐文件、封面、歌词和歌单，不依赖账号或流媒体目录。",
  },
  {
    title: "桌面优先",
    body: "面向 macOS、Windows 和 Linux 的个人音乐管理与播放体验。",
  },
];

export function SettingsView({ onClose }: { readonly onClose: () => void }) {
  const { theme, setTheme } = useTheme();
  const status = useLibraryStatus();
  const [closeBehavior, setCloseBehavior] = useState<CloseBehavior>("background");
  const [choosing, setChoosing] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const panelRef = useRef<HTMLElement>(null);
  // A `Picker`-tier layer: Escape closes it before the menus beneath it, focus is
  // trapped inside the panel and restored to 设置 on close (task 12.1).
  useOverlay({ tier: OverlayTier.Picker, onClose, containerRef: panelRef });
  useFocusTrap(panelRef);

  function chooseTheme(next: Theme) {
    // setTheme persists + stamps data-echo-theme; selection semantics update.
    setTheme(next);
    setNotice(`${THEME_OPTIONS.find((option) => option.value === next)?.name ?? ""} 已应用`);
  }

  function chooseClose(next: CloseBehavior) {
    setCloseBehavior(next);
    void bridge.call("set_close_behavior", { behavior: next });
  }

  async function chooseDirectory() {
    setChoosing(true);
    try {
      const result = (await bridge.call("choose_library_root")) as unknown | null;
      // A cancelled picker is a genuine no-op — the dialog stays open and the
      // current directory keeps being reported.
      if (result !== null) await status.refresh();
    } catch {
      setNotice("切换音乐目录失败，已保留当前目录");
    } finally {
      setChoosing(false);
    }
  }

  return (
    <section
      className="settings-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-heading"
      data-testid="settings"
      ref={panelRef}
    >
      <div className="settings-panel">
        <header className="settings-dialog-head">
          <div>
            <p className="eyebrow">Echo 偏好</p>
            <h1 id="settings-heading" tabIndex={-1}>
              设置
            </h1>
          </div>
          <button type="button" className="settings-close" aria-label="关闭设置" onClick={onClose}>
            <Icon name="close" />
          </button>
        </header>

        <div className="settings-scroll">
          <header className="settings-intro">
            <p>管理本机音乐资料库与应用外观。所有更改仅保存在这台设备上。</p>
          </header>

          <section className="settings-section">
            <header className="settings-section-head">
              <h2>本地存储</h2>
              <p>选择 Echo 用于扫描、索引和管理音乐文件的文件夹。</p>
            </header>
            <div className="setting-row">
              <div className="storage-value">
                <strong>音乐目录</strong>
                <span
                  className={`storage-path${status.activeRoot ? "" : " is-empty"}`}
                  title={status.activeRoot ?? "尚未选择音乐目录"}
                >
                  {status.activeRoot ?? "尚未选择音乐目录"}
                </span>
                <span className="setting-help">桌面端专属设置；移动端默认使用系统媒体库。</span>
              </div>
              <button
                type="button"
                className="btn btn-primary"
                disabled={choosing}
                onClick={() => void chooseDirectory()}
                data-testid="storage-directory-button"
              >
                {choosing ? "正在处理…" : "选择文件夹"}
              </button>
            </div>
            <p className="setting-note">
              <Icon name="info" />
              <span>更改目录不会移动或删除现有音乐；Echo 会在下次扫描时更新资料库。</span>
            </p>
          </section>

          <section className="settings-section">
            <header className="settings-section-head">
              <h2>主题</h2>
              <p>主题仅影响强调色与主要操作，保持纯白的音乐工作区。</p>
            </header>
            <div className="theme-choices" role="group" aria-label="选择主题">
              {THEME_OPTIONS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className="theme-option setting-theme-option"
                  aria-pressed={option.value === theme}
                  data-theme={option.value}
                  data-theme-name={option.name}
                  data-echo-theme-option={option.value}
                  onClick={() => chooseTheme(option.value)}
                >
                  <span className={`theme-swatch ${option.swatch}`} />
                  <span className="theme-option-copy">
                    <strong>{option.name}</strong>
                    <span>{option.hint}</span>
                  </span>
                </button>
              ))}
            </div>
          </section>

          <section className="settings-section">
            <header className="settings-section-head">
              <h2>应用行为</h2>
              <p>关闭主窗口时 Echo 如何结束。</p>
            </header>
            <div className="setting-row">
              <div className="storage-value">
                <strong>关闭主窗口时</strong>
                <span className="setting-help">
                  选择退出应用，或留在后台继续播放；桌面端专属设置。
                </span>
              </div>
              <div className="setting-choices" role="radiogroup" aria-label="关闭主窗口时">
                <label className="setting-choice">
                  <input
                    type="radio"
                    name="close-behavior"
                    checked={closeBehavior === "background"}
                    onChange={() => chooseClose("background")}
                  />
                  保持在后台运行
                </label>
                <label className="setting-choice">
                  <input
                    type="radio"
                    name="close-behavior"
                    checked={closeBehavior === "exit"}
                    onChange={() => chooseClose("exit")}
                  />
                  退出应用
                </label>
              </div>
            </div>
          </section>

          <section className="settings-section">
            <header className="settings-section-head">
              <h2>关于 Echo</h2>
              <p>一个本地优先的音乐播放器，为自己的音乐资料库而设计。</p>
            </header>
            <div className="about-list">
              {ABOUT.map((item) => (
                <div className="about-item" key={item.title}>
                  <strong>{item.title}</strong>
                  <span>{item.body}</span>
                </div>
              ))}
            </div>
          </section>

          {notice ? (
            <p className="settings-note" role="status">
              {notice}
            </p>
          ) : null}
        </div>
      </div>
    </section>
  );
}
