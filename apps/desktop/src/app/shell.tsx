/**
 * Shell navigation context + the workspace topbar.
 *
 * The prototype keeps the narrow-screen sidebar toggle as the first child of
 * `.topbar`, which lives inside whichever view is routed into the workspace.
 * Rather than threading the toggle callback through every view, `App` publishes
 * it on this context and the views render `<Topbar>` — the same DOM the
 * prototype has, with the sidebar state owned in one place.
 */

import { createContext, useContext, type ReactNode } from "react";

import { Icon } from "./Icon";

export interface ShellNav {
  readonly sidebarOpen: boolean;
  readonly onToggleSidebar: () => void;
}

const ShellNavContext = createContext<ShellNav | null>(null);

export const ShellNavProvider = ShellNavContext.Provider;

export function useShellNav(): ShellNav {
  const nav = useContext(ShellNavContext);
  if (!nav) {
    throw new Error("useShellNav must be used inside the app shell");
  }
  return nav;
}

/** The shell's navigation state, or null when rendered outside the shell. */
function useOptionalShellNav(): ShellNav | null {
  return useContext(ShellNavContext);
}

/**
 * Narrow-screen sidebar toggle (`≤760px` only — the stylesheet hides it on wide
 * windows). `aria-expanded` reflects the sidebar state and focus returns here
 * when the sidebar closes.
 */
export function SidebarToggle() {
  const { sidebarOpen, onToggleSidebar } = useShellNav();
  return (
    <button
      type="button"
      className="icon-button sidebar-toggle"
      aria-label={sidebarOpen ? "收起侧边栏" : "展开侧边栏"}
      title={sidebarOpen ? "收起侧边栏" : "展开侧边栏"}
      aria-expanded={sidebarOpen}
      aria-controls="app-sidebar"
      onClick={onToggleSidebar}
      data-testid="sidebar-toggle"
    >
      <Icon className="sidebar-closed-icon" name="sidebar" />
      <Icon className="sidebar-open-icon" name="menu" />
    </button>
  );
}

/** 工作区顶栏: sidebar toggle + breadcrumb + view-specific controls. */
export function Topbar(props: { readonly title: string; readonly children?: ReactNode }) {
  // The toggle only exists inside the shell (it needs the shell's sidebar
  // state). A view rendered on its own — as the component tests do — still gets
  // the prototype's breadcrumb without it.
  const nav = useOptionalShellNav();
  return (
    <header className="topbar" data-od-id="workspace-topbar">
      {nav ? <SidebarToggle /> : null}
      <div className="crumb">
        <strong>资料库</strong> / <span>{props.title}</span>
      </div>
      {props.children}
    </header>
  );
}
