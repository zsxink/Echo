import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it } from "vitest";

import { SettingsView } from "./SettingsView";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke(command: string, value: unknown): void };
  }
).__echoTest;

beforeEach(() => {
  localStorage.clear();
  mocks.setInvoke("get_close_behavior", "exit");
  mocks.setInvoke("library_status", {
    configured: true,
    readOnly: false,
    unavailable: false,
    scanning: false,
    activeRoot: "root-1",
  });
  mocks.setInvoke("start_scan", {
    generation: 2,
    cancelled: false,
    progress: { state: "completed" },
  });
});

describe("SettingsView close behavior", () => {
  it("shows the persisted exit preference instead of the background default", async () => {
    render(<SettingsView onClose={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByLabelText("退出应用")).toBeChecked();
    });
    expect(screen.getByLabelText("保持在后台运行")).not.toBeChecked();
  });
});

describe("SettingsView library rescan", () => {
  it("starts a scan for the active library and reports completion", async () => {
    render(<SettingsView onClose={() => undefined} />);

    const button = await screen.findByTestId("rescan-library-button");
    fireEvent.click(button);

    await waitFor(() => {
      expect(button).toHaveTextContent("重新扫描");
    });
    const scanCall = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.find(
      ([command]) => command === "start_scan",
    );
    expect(scanCall).toEqual(["start_scan", { root: "root-1" }]);
    expect(screen.getByRole("status")).toHaveTextContent("资料库已重新扫描");
  });

  it("disables rescan when no library root is configured", async () => {
    mocks.setInvoke("library_status", {
      configured: false,
      readOnly: false,
      unavailable: false,
      scanning: false,
    });
    render(<SettingsView onClose={() => undefined} />);

    expect(await screen.findByTestId("rescan-library-button")).toBeDisabled();
  });

  it("disables rescan while another scan is already running", async () => {
    mocks.setInvoke("library_status", {
      configured: true,
      readOnly: false,
      unavailable: false,
      scanning: true,
      activeRoot: "root-1",
    });
    render(<SettingsView onClose={() => undefined} />);

    const button = await screen.findByTestId("rescan-library-button");
    expect(button).toBeDisabled();
    expect(button).toHaveTextContent("正在扫描");
  });

  it("disables rescan while the directory picker is active", async () => {
    let resolvePicker!: (value: null) => void;
    mocks.setInvoke("choose_library_root", new Promise<null>((resolve) => (resolvePicker = resolve)));
    render(<SettingsView onClose={() => undefined} />);

    act(() => fireEvent.click(screen.getByTestId("storage-directory-button")));
    expect(await screen.findByTestId("rescan-library-button")).toBeDisabled();
    act(() => resolvePicker(null));
  });

  it("reports a scan failure without claiming that metadata was updated", async () => {
    mocks.setInvoke("start_scan", new Error("scan failed"));
    render(<SettingsView onClose={() => undefined} />);

    fireEvent.click(await screen.findByTestId("rescan-library-button"));

    expect(await screen.findByRole("status")).toHaveTextContent("资料库扫描失败");
  });
});
