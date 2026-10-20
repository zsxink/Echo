/**
 * 设置 (task 10.1 theme UI; close behavior surfaced from the desktop preference).
 *
 * Reproduces the prototype's `.settings-dialog`: a centred `.settings-panel`
 * with a full-height settings sidebar and a right-side `.settings-main` containing
 * the module breadcrumb, close button, and scrolling `.settings-scroll`.
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

import { useEffect, useRef, useState, type KeyboardEvent } from "react";

import { bridge } from "../../bridge";
import { invalidateCovers } from "../../app/coverArt";
import { Icon } from "../../app/Icon";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import type { CloseBehavior } from "../../ipc/ipc-types.generated";
import { invalidateLibrary } from "../library";
import { useLibraryStatus } from "../workspace";
import { useTheme, type Theme } from "./useTheme";
import { LanSyncSettings } from "../sync/LanSyncSettings";

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
  { value: "coral", swatch: "wine", name: "珊瑚玫红", hint: "明快、温暖" },
  { value: "turquoise", swatch: "green", name: "松石绿", hint: "平衡、舒缓" },
];

const SETTINGS_MODULES = [
  { id: "general", label: "通用", icon: "sliders" },
  { id: "sync", label: "同步", icon: "sync" },
  { id: "shortcuts", label: "快捷键", icon: "keyboard" },
  { id: "about", label: "关于", icon: "info" },
] as const;

const SHORTCUTS = [
  { action: "播放 / 暂停", keys: ["Space"] },
  { action: "快退 / 快进 5 秒", keys: ["←", "→"] },
  { action: "降低 / 提高音量", keys: ["↓", "↑"] },
  { action: "静音 / 取消静音", keys: ["M"] },
  { action: "上一首", keys: [","] },
  { action: "下一首", keys: ["."] },
] as const;

export function SettingsView({ onClose }: { readonly onClose: () => void }) {
  const { theme, setTheme } = useTheme();
  const status = useLibraryStatus();
  const [closeBehavior, setCloseBehavior] = useState<CloseBehavior>("background");
  const [choosing, setChoosing] = useState(false);
  const [rescanning, setRescanning] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [activeModule, setActiveModule] =
    useState<(typeof SETTINGS_MODULES)[number]["id"]>("general");
  const panelRef = useRef<HTMLElement>(null);
  const navigationRef = useRef<HTMLDivElement>(null);
  // A `Picker`-tier layer: Escape closes it before the menus beneath it, focus is
  // trapped inside the panel and restored to 设置 on close (task 12.1).
  useOverlay({ tier: OverlayTier.Picker, onClose, containerRef: panelRef });
  useFocusTrap(panelRef);

  useEffect(() => {
    const navigation = navigationRef.current;
    if (!navigation) return;
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(max-width: 760px)");
    const updateOrientation = () =>
      navigation.setAttribute("aria-orientation", media.matches ? "horizontal" : "vertical");
    updateOrientation();
    media.addEventListener("change", updateOrientation);
    return () => media.removeEventListener("change", updateOrientation);
  }, []);

  useEffect(() => {
    let current = true;
    void bridge
      .call("get_close_behavior")
      .then((behavior) => {
        if (current && (behavior === "background" || behavior === "exit")) {
          setCloseBehavior(behavior);
        }
      })
      .catch(() => {
        if (current) setNotice("无法读取关闭行为，暂按当前显示设置");
      });
    return () => {
      current = false;
    };
  }, []);

  function chooseTheme(next: Theme) {
    // setTheme persists + stamps data-echo-theme; selection semantics update.
    setTheme(next);
    setNotice(`${THEME_OPTIONS.find((option) => option.value === next)?.name ?? ""} 已应用`);
  }

  async function chooseClose(next: CloseBehavior) {
    const previous = closeBehavior;
    setCloseBehavior(next);
    try {
      await bridge.call("set_close_behavior", { behavior: next });
    } catch {
      setCloseBehavior(previous);
      setNotice("关闭行为保存失败，已恢复原设置");
    }
  }

  async function chooseDirectory() {
    setChoosing(true);
    try {
      const result = await bridge.call("choose_library_root");
      // A cancelled picker is a genuine no-op — the dialog stays open and the
      // current directory keeps being reported.
      if (result !== null) await status.refresh();
    } catch {
      setNotice("切换音乐目录失败，已保留当前目录");
    } finally {
      setChoosing(false);
    }
  }

  async function rescanLibrary() {
    const root = status.activeRoot;
    if (!root || status.scanning || rescanning) return;

    setRescanning(true);
    setNotice(null);
    try {
      await bridge.call("start_scan", { root });
      // A rescan can keep the same SongId while replacing metadata and its
      // embedded cover. Refresh every mounted surface after the committed scan.
      invalidateCovers();
      invalidateLibrary();
      await status.refresh();
      setNotice("音乐库已重新扫描，封面、歌词和标签已更新");
    } catch {
      setNotice("音乐库扫描失败，请稍后重试");
    } finally {
      setRescanning(false);
    }
  }

  function handleModuleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const currentIndex = SETTINGS_MODULES.findIndex((module) => module.id === activeModule);
    let nextIndex = currentIndex;
    if (event.key === "ArrowRight" || event.key === "ArrowDown")
      nextIndex = (currentIndex + 1) % SETTINGS_MODULES.length;
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp")
      nextIndex = (currentIndex - 1 + SETTINGS_MODULES.length) % SETTINGS_MODULES.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = SETTINGS_MODULES.length - 1;
    else return;
    event.preventDefault();
    const next = SETTINGS_MODULES[nextIndex];
    setActiveModule(next.id);
    document.getElementById(`settings-tab-${next.id}`)?.focus();
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
        <div className="settings-workspace">
          <aside className="settings-sidebar" aria-label="设置侧边栏">
            <div className="settings-brand" aria-label="Echo">
              <span className="settings-brand-mark" aria-hidden="true">
                <Icon name="note" />
              </span>
              <span>Echo</span>
            </div>
            <div
              className="settings-nav"
              ref={navigationRef}
              role="tablist"
              aria-label="设置模块"
              aria-orientation="vertical"
              onKeyDown={handleModuleKeyDown}
            >
              {SETTINGS_MODULES.map((module) => (
                <button
                  key={module.id}
                  id={`settings-tab-${module.id}`}
                  type="button"
                  role="tab"
                  aria-selected={activeModule === module.id}
                  aria-controls={`settings-panel-${module.id}`}
                  tabIndex={activeModule === module.id ? 0 : -1}
                  onClick={() => setActiveModule(module.id)}
                >
                  <span className="settings-nav-icon" aria-hidden="true">
                    <Icon name={module.icon} />
                  </span>
                  {module.label}
                </button>
              ))}
            </div>
          </aside>
          <div className="settings-main">
            <header className="settings-dialog-head">
              <h1 className="settings-breadcrumb" id="settings-heading" tabIndex={-1}>
                <span className="settings-breadcrumb-root">设置</span>
                <span className="settings-breadcrumb-separator" aria-hidden="true">
                  /
                </span>
                <strong className="settings-breadcrumb-current">
                  {SETTINGS_MODULES.find((module) => module.id === activeModule)?.label ?? "通用"}
                </strong>
              </h1>
              <button
                type="button"
                className="settings-close"
                aria-label="关闭设置"
                onClick={onClose}
              >
                <Icon name="close" />
              </button>
            </header>
            <div className="settings-scroll">
              {notice ? (
                <p className="settings-note" role="status">
                  {notice}
                </p>
              ) : null}
              <section
                className="settings-module"
                id="settings-panel-general"
                role="tabpanel"
                aria-labelledby="settings-tab-general"
                tabIndex={0}
                aria-hidden={activeModule !== "general"}
                hidden={activeModule !== "general"}
              >
                <header className="settings-module-head">
                  <h2>通用</h2>
                  <p>管理本地音乐库、外观和应用行为。</p>
                </header>
                <section className="settings-section">
                  <header className="settings-section-head">
                    <h2>本地存储</h2>
                    <p>选择 Echo 用于扫描、索引和管理音乐文件的文件夹。</p>
                  </header>
                  <div className="setting-row">
                    <div className="storage-value">
                      <strong>音乐库目录</strong>
                      <span
                        className={`storage-path${status.activeRootPath ? "" : " is-empty"}`}
                        title={status.activeRootPath ?? "尚未选择音乐目录"}
                      >
                        {status.activeRootPath ?? "尚未选择音乐目录"}
                      </span>
                    </div>
                    <button
                      type="button"
                      className="btn btn-primary"
                      disabled={choosing || status.scanning || rescanning}
                      onClick={() => void chooseDirectory()}
                      data-testid="storage-directory-button"
                    >
                      {choosing ? "正在处理…" : "选择文件夹"}
                    </button>
                  </div>
                  <p className="setting-note">
                    <Icon name="info" />
                    <span>更改目录不会移动或删除现有音乐；Echo 会在下次扫描时更新音乐库。</span>
                  </p>
                  <div className="setting-row">
                    <div className="storage-value">
                      <strong>重新扫描音乐库</strong>
                      <span className="setting-help">
                        检查并更新已变化音频文件中的封面、歌词和标签，不会移动或删除原始文件。
                      </span>
                    </div>
                    <button
                      type="button"
                      className="btn"
                      disabled={!status.activeRoot || status.scanning || rescanning || choosing}
                      onClick={() => void rescanLibrary()}
                      data-testid="rescan-library-button"
                    >
                      {status.scanning || rescanning ? "正在扫描…" : "重新扫描"}
                    </button>
                  </div>
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
                        最小化到托盘可继续在后台播放，也可以直接退出应用。
                      </span>
                    </div>
                    <div
                      className="setting-choices close-behavior-choices"
                      role="radiogroup"
                      aria-label="关闭主窗口时"
                    >
                      <label className="setting-choice close-behavior-choice">
                        <input
                          type="radio"
                          name="close-behavior"
                          checked={closeBehavior === "background"}
                          onChange={() => void chooseClose("background")}
                        />
                        最小化到托盘
                      </label>
                      <label className="setting-choice close-behavior-choice">
                        <input
                          type="radio"
                          name="close-behavior"
                          checked={closeBehavior === "exit"}
                          onChange={() => void chooseClose("exit")}
                        />
                        退出应用
                      </label>
                    </div>
                  </div>
                </section>
              </section>
              <section
                className="settings-module"
                id="settings-panel-shortcuts"
                role="tabpanel"
                aria-labelledby="settings-tab-shortcuts"
                tabIndex={0}
                aria-hidden={activeModule !== "shortcuts"}
                hidden={activeModule !== "shortcuts"}
              >
                <header className="settings-module-head">
                  <h2>快捷键</h2>
                  <p>在 Echo 中使用键盘控制播放。</p>
                </header>
                <div className="shortcut-list" aria-label="当前播放快捷键">
                  {SHORTCUTS.map((shortcut) => (
                    <div className="shortcut-row" key={shortcut.action}>
                      <span>{shortcut.action}</span>
                      <span className="shortcut-keys">
                        {shortcut.keys.map((key) => (
                          <kbd className="shortcut-key" key={key}>
                            {key}
                          </kbd>
                        ))}
                      </span>
                    </div>
                  ))}
                </div>
                <p className="shortcut-note">
                  在输入框中输入时快捷键不会触发；打开播放队列或音效面板时，方向键和字母键由当前界面处理。
                </p>
              </section>

              <section
                className="settings-module"
                id="settings-panel-sync"
                role="tabpanel"
                aria-labelledby="settings-tab-sync"
                tabIndex={0}
                aria-hidden={activeModule !== "sync"}
                hidden={activeModule !== "sync"}
              >
                <header className="settings-module-head">
                  <h2>同步</h2>
                  <p>管理设备配对、同步状态和冲突处理。</p>
                </header>
                <LanSyncSettings
                  libraryRoot={status.activeRoot}
                  libraryRootPath={status.activeRootPath}
                />
              </section>
              <section
                className="settings-module settings-module-about"
                id="settings-panel-about"
                role="tabpanel"
                aria-labelledby="settings-tab-about"
                tabIndex={0}
                aria-hidden={activeModule !== "about"}
                hidden={activeModule !== "about"}
              >
                <div className="settings-about-pane">
                  <span className="settings-about-mark" aria-hidden="true">
                    <Icon name="note" />
                  </span>
                  <p>专注于本地音乐体验</p>
                  <div className="settings-about-copy">
                    <p className="settings-about-lead">
                      一个本地优先的音乐播放器，为自己的音乐库而设计。
                    </p>
                    <p>
                      <strong>本地优先</strong>
                      管理自己的音乐文件、封面、歌词和歌单，不依赖账号或流媒体目录。
                    </p>
                    <p>
                      <strong>桌面优先</strong>
                      面向 macOS、Windows 和 Linux 的个人音乐管理与播放体验。
                    </p>
                  </div>
                </div>
              </section>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
