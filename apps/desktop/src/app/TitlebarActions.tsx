/**
 * 标题栏常驻动作：导入 / 设置（顺序固定，导入在前）。
 *
 * 桌面平台把这两个入口放进顶部安全区条（`--titlebar-inset`，与窗口控件同一条
 * 28px 带）：Windows 排在自绘窗口控件（`.window-controls`）的左侧；macOS 那
 * 一条是交通灯预留带，右上角没有原生控件，按钮就靠右上角摆放。侧边栏品牌区
 * 的同一组入口（playlist-search-locate-import 5.2）保持不变——那是规格落点，
 * 这里是桌面壳层的补强，两者共用同一组回调。
 *
 * 平台门与 `WindowControls` 同源：只有 windows / macos 渲染（浏览器预览与
 * jsdom 保持原型一致）。样式见 `.titlebar-actions`（app-extras.css）：无背景
 * 无边框，悬浮只加深图标颜色，图标尺寸与窗口控件一致。
 */

import { Icon } from "./Icon";
import { currentPlatform } from "./platform";

export function TitlebarActions(props: {
  readonly importing: boolean;
  /** 资料库只读时没有导入入口（与侧边栏品牌区同一规则）。 */
  readonly showImport: boolean;
  readonly onImport: () => void;
  readonly onSettings: () => void;
}) {
  const platform = currentPlatform();
  if (platform !== "windows" && platform !== "macos") {
    return null;
  }
  return (
    <div className="titlebar-actions" data-testid="titlebar-actions">
      {props.showImport ? (
        <button
          type="button"
          className="titlebar-action"
          aria-label={props.importing ? "正在导入" : "导入"}
          title={props.importing ? "正在导入" : "导入"}
          aria-busy={props.importing}
          disabled={props.importing}
          onClick={props.onImport}
          data-testid="titlebar-import"
        >
          <Icon name="import" />
        </button>
      ) : null}
      <button
        type="button"
        className="titlebar-action"
        aria-label="设置"
        title="设置"
        onClick={props.onSettings}
        data-testid="titlebar-settings"
      >
        <Icon name="settingsNut" />
      </button>
    </div>
  );
}
