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
  mocks.setInvoke("sync_status", syncStatus());
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
    mocks.setInvoke(
      "choose_library_root",
      new Promise<null>((resolve) => (resolvePicker = resolve)),
    );
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

describe("SettingsView LAN sync preferences", () => {
  it("reads the native disabled state and keeps the service opt-in", async () => {
    render(<SettingsView onClose={() => undefined} />);

    await screen.findByLabelText("退出应用");
    expect(screen.getByTestId("lan-sync-state")).toHaveTextContent("已关闭");
    expect(screen.queryByTestId("lan-sync-start")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "主机" }));
    fireEvent.change(screen.getByPlaceholderText("192.168.1.20"), {
      target: { value: "192.168.1.20" },
    });
    expect(screen.getByTestId("lan-sync-start")).toBeEnabled();
  });

  it("shows the native provisioning failure without claiming a connection", async () => {
    mocks.setInvoke("host_start", new Error("sync.unconfigured"));
    render(<SettingsView onClose={() => undefined} />);

    await screen.findByLabelText("退出应用");
    fireEvent.click(screen.getByRole("radio", { name: "主机" }));
    fireEvent.change(screen.getByPlaceholderText("192.168.1.20"), {
      target: { value: "192.168.1.20" },
    });
    fireEvent.click(screen.getByTestId("lan-sync-start"));

    expect(await screen.findByRole("alert")).toHaveTextContent("同步服务尚未完成安全配置");
    expect(screen.getByTestId("lan-sync-state")).toHaveTextContent("已关闭");
    expect(invoke).toHaveBeenCalledWith("host_start", { address: "192.168.1.20" });
  });

  it("renders hosts returned by native LAN discovery", async () => {
    mocks.setInvoke("discover_hosts", [
      {
        name: "Echo 主机",
        addresses: ["192.168.1.10"],
        port: 43000,
        deviceId: "device-1",
        protocolVersion: "1",
      },
    ]);
    render(<SettingsView onClose={() => undefined} />);

    await screen.findByLabelText("退出应用");
    fireEvent.click(screen.getByRole("button", { name: "搜索局域网主机" }));

    expect(await screen.findByTestId("lan-sync-hosts")).toHaveTextContent("Echo 主机");
    expect(screen.getByTestId("lan-sync-hosts")).toHaveTextContent("192.168.1.10:43000");
  });

  it("lets a peer join with a host IP and six-digit pairing code", async () => {
    mocks.setInvoke("discover_hosts", [
      {
        name: "Echo 主机",
        addresses: ["192.168.1.10"],
        port: 43000,
        deviceId: "device-1",
        protocolVersion: "1",
      },
    ]);
    mocks.setInvoke("request_sync_pairing", {
      enabled: true,
      role: "peer",
      state: "connected",
      sessionId: 1,
      eventSequence: 1,
      libraryId: "library-1",
      address: "https://192.168.1.10:50826",
      lastError: null,
    });
    render(<SettingsView onClose={() => undefined} />);

    await screen.findByLabelText("退出应用");
    fireEvent.click(screen.getByRole("button", { name: "搜索局域网主机" }));
    fireEvent.click(await screen.findByRole("button", { name: "使用主机 Echo 主机 的地址" }));
    // The label also carries the help text, so match by substring.
    expect(screen.getByLabelText(/主机 IP/)).toHaveValue("192.168.1.10");

    fireEvent.change(screen.getByLabelText(/六位验证码/), {
      target: { value: "123456" },
    });
    fireEvent.click(screen.getByRole("button", { name: "加入" }));

    await screen.findByText("已连接");
    expect(invoke).toHaveBeenCalledWith("request_sync_pairing", {
      code: "123456",
      hostAddress: "192.168.1.10",
    });
  });
});

function syncStatus() {
  return {
    enabled: false,
    role: "none",
    state: "disabled",
    sessionId: 0,
    eventSequence: 0,
    libraryId: null,
    address: null,
    lastError: null,
  };
}
