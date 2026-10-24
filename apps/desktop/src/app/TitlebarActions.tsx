/**
 * 标题栏常驻动作：导入 / 设置（顺序固定，导入在前）。
 *
 * 桌面平台把这两个入口放进顶部标题栏条：按钮固定 28×28、8px 圆角，与窗口
 * 控件同款——Windows 排在自绘窗口控件（`.window-controls`）左侧，macOS 那
 * 一条是交通灯预留带，右上角没有原生控件，按钮就贴右上角摆放。品牌区悬停
 * 入口已移除（规格同步改为标题栏条承载，见 desktop-app-shell spec），这里
 * 是唯一的导入/设置常驻入口；沉浸模式整条隐藏。
 *
 * 平台门与 `WindowControls` 同源：只有 windows / macos 渲染（浏览器预览与
 * jsdom 保持原型一致）。样式见 `.titlebar-actions`（app-extras.css）：悬浮/
 * 聚焦亮起与窗口控件同款底色，40ms 过渡。
 */

import { Icon } from "./Icon";
import { currentPlatform } from "./platform";

export function TitlebarActions(props: {
  readonly importing: boolean;
  /** 资料库只读时没有导入入口（既有规则，随入口迁移至标题栏条沿用）。 */
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
