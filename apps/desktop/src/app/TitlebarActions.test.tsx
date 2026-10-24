/**
 * TitlebarActions — the 导入/设置 pair in the desktop titlebar strip.
 *
 * Platform gate mirrors WindowControls: rendered on Windows and macOS only
 * (the two platforms whose chrome the shell draws/manages), nothing in a plain
 * browser preview or jsdom unless the platform dataset is preset. The buttons
 * delegate to the same callbacks the sidebar brand row uses — these tests
 * assert the wiring and the 导入-before-设置 order, not the actions themselves.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TitlebarActions } from "./TitlebarActions";

function setPlatform(platform: string | undefined): void {
  if (platform === undefined) {
    delete document.documentElement.dataset.platform;
  } else {
    document.documentElement.dataset.platform = platform;
  }
}

function renderActions(overrides?: Partial<Parameters<typeof TitlebarActions>[0]>) {
  const props = {
    importing: false,
    showImport: true,
    onImport: vi.fn(),
    onSettings: vi.fn(),
    ...overrides,
  };
  const view = render(<TitlebarActions {...props} />);
  return { props, view };
}

describe("TitlebarActions", () => {
  beforeEach(() => {
    setPlatform(undefined);
  });

  it("renders nothing outside the desktop platforms", () => {
    for (const platform of ["linux", "web", undefined]) {
      setPlatform(platform);
      const { unmount } = render(
        <TitlebarActions importing={false} showImport onImport={() => {}} onSettings={() => {}} />,
      );
      expect(screen.queryByTestId("titlebar-actions")).not.toBeInTheDocument();
      unmount();
    }
  });

  it("renders 导入 before 设置 on Windows and macOS", () => {
    for (const platform of ["windows", "macos"]) {
      setPlatform(platform);
      const { view } = renderActions();
      const actions = screen.getByTestId("titlebar-actions");
      const labels = Array.from(actions.querySelectorAll("button")).map((button) =>
        button.getAttribute("aria-label"),
      );
      expect(labels).toEqual(["导入", "设置"]);
      view.unmount();
    }
  });

  it("wires the buttons to the shell's import and settings callbacks", () => {
    setPlatform("windows");
    const { props } = renderActions();
    fireEvent.click(screen.getByTestId("titlebar-import"));
    fireEvent.click(screen.getByTestId("titlebar-settings"));
    expect(props.onImport).toHaveBeenCalledTimes(1);
    expect(props.onSettings).toHaveBeenCalledTimes(1);
  });

  it("hides 导入 on a read-only library but keeps 设置", () => {
    setPlatform("macos");
    renderActions({ showImport: false });
    expect(screen.queryByTestId("titlebar-import")).not.toBeInTheDocument();
    expect(screen.getByTestId("titlebar-settings")).toBeInTheDocument();
  });

  it("reflects the importing state on the 导入 button", () => {
    setPlatform("windows");
    renderActions({ importing: true });
    const importing = screen.getByTestId("titlebar-import");
    expect(importing).toBeDisabled();
    expect(importing).toHaveAccessibleName("正在导入");
    expect(importing).toHaveAttribute("aria-busy", "true");
  });
});
