/**
 * Settings surface (task 10.1 theme UI; close behavior surfaced from the
 * desktop preference).
 *
 * Presents the three accent themes (coral default, cobalt, turquoise) as
 * selectable options with live selected semantics, and the "关闭主窗口时"
 * exit/background choice. The favorite-heart red and other status semantics
 * are not part of the theme choice and are never overridden.
 */

import { useState } from "react";

import { bridge } from "../../bridge";
import { THEMES, useTheme, type Theme } from "./useTheme";
import type { CloseBehavior } from "../../ipc/ipc-types.generated";

const THEME_LABELS: Record<Theme, string> = {
  coral: "珊瑚玫红（默认）",
  cobalt: "深钴蓝",
  turquoise: "松石绿",
};

export function SettingsView() {
  const { theme, setTheme } = useTheme();
  const [closeBehavior, setCloseBehavior] = useState<CloseBehavior>("background");
  const [notice, setNotice] = useState<string | null>(null);

  function chooseTheme(next: Theme) {
    // setTheme persists + stamps data-echo-theme; selection semantics update.
    setTheme(next);
    setNotice(`${THEME_LABELS[next]} 已应用`);
  }

  function chooseClose(next: CloseBehavior) {
    setCloseBehavior(next);
    void bridge.call("set_close_behavior", { behavior: next });
  }

  return (
    <div className="settings" data-testid="settings">
      <h2 className="workspace-title">设置</h2>

      <section className="settings-section">
        <h3 className="settings-heading">主题</h3>
        <div className="theme-row" role="radiogroup" aria-label="主题">
          {THEMES.map((value) => (
            <button
              key={value}
              type="button"
              role="radio"
              aria-checked={value === theme}
              className={`theme-option${value === theme ? " is-active" : ""}`}
              onClick={() => chooseTheme(value)}
              data-echo-theme-option={value}
            >
              <span className="theme-swatch" aria-hidden="true" />
              <span className="theme-label">{THEME_LABELS[value]}</span>
            </button>
          ))}
        </div>
      </section>

      <section className="settings-section">
        <h3 className="settings-heading">关闭主窗口时</h3>
        <div className="close-behavior" role="radiogroup" aria-label="关闭主窗口时">
          <label>
            <input
              type="radio"
              name="close"
              checked={closeBehavior === "background"}
              onChange={() => chooseClose("background")}
            />
            保持在后台运行
          </label>
          <label>
            <input
              type="radio"
              name="close"
              checked={closeBehavior === "exit"}
              onChange={() => chooseClose("exit")}
            />
            退出应用
          </label>
        </div>
      </section>

      {notice ? (
        <p className="settings-notice" role="status">
          {notice}
        </p>
      ) : null}
    </div>
  );
}
