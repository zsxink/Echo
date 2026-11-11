import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { bridge } from "../../bridge";
import type { NormalizationSnapshotDto } from "../../ipc/ipc-types.generated";
import { NormalizationControls } from "./NormalizationControls";

vi.mock("../../bridge", () => ({ bridge: { call: vi.fn() } }));
const call = vi.mocked(bridge.call);

function snapshot(patch: Partial<NormalizationSnapshotDto> = {}): NormalizationSnapshotDto {
  return {
    preference: { schemaVersion: 1, enabled: false, mode: "track", preampDb: 0 },
    runtime: { gainDb: 0, status: "disabled", reason: null },
    ...patch,
  };
}

function renderWith(initial: NormalizationSnapshotDto) {
  call.mockResolvedValue(initial);
  render(<NormalizationControls />);
}

function switchInput(): HTMLInputElement {
  return screen.getByRole("switch", { name: "启用音量归一化" }) as HTMLInputElement;
}

beforeEach(() => {
  call.mockReset();
});

describe("loudness normalization controls", () => {
  it("defaults to off and explains the honest scope", async () => {
    renderWith(snapshot());
    await waitFor(() => expect(switchInput().checked).toBe(false));
    expect(screen.getByTestId("normalization-status").textContent).toContain("已关闭");
    expect(screen.getByText(/sample-peak/)).toBeTruthy();
    expect(screen.queryByText(/提升音质/)).toBeNull();
  });

  it("enabling publishes a typed preference and shows the applied status", async () => {
    const enabled = snapshot({
      preference: { schemaVersion: 1, enabled: true, mode: "track", preampDb: 0 },
      runtime: { gainDb: -6, status: "applied", reason: null },
    });
    renderWith(snapshot());
    await screen.findByTestId("normalization-status");
    call.mockResolvedValueOnce(enabled);
    fireEvent.click(switchInput());
    await waitFor(() =>
      expect(screen.getByTestId("normalization-status").textContent).toContain("按文件内标签"),
    );
    expect(call).toHaveBeenCalledWith("set_loudness_normalization", {
      preference: expect.objectContaining({ enabled: true, schemaVersion: 1 }),
    });
  });

  it("changes mode and preamp through the same preference document", async () => {
    renderWith(snapshot());
    await screen.findByTestId("normalization-status");
    call.mockResolvedValueOnce(
      snapshot({
        preference: { schemaVersion: 1, enabled: false, mode: "album", preampDb: 2 },
      }),
    );
    fireEvent.change(screen.getByLabelText("归一化依据"), { target: { value: "album" } });
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("set_loudness_normalization", {
        preference: expect.objectContaining({ mode: "album" }),
      }),
    );
    call.mockResolvedValueOnce(
      snapshot({
        preference: { schemaVersion: 1, enabled: false, mode: "track", preampDb: 1.5 },
      }),
    );
    fireEvent.change(screen.getByLabelText(/前置增益/), { target: { value: "1.5" } });
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("set_loudness_normalization", {
        preference: expect.objectContaining({ preampDb: 1.5 }),
      }),
    );
  });

  it("explains missing tags and clipping without claiming measurement", async () => {
    renderWith(
      snapshot({
        preference: { schemaVersion: 1, enabled: true, mode: "track", preampDb: 0 },
        runtime: { gainDb: 0, status: "noTags", reason: null },
      }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("normalization-status").textContent).toContain("无可用响度标签"),
    );
  });

  it("shows the clipping reason when the peak clamp reduced the gain", async () => {
    renderWith(
      snapshot({
        preference: { schemaVersion: 1, enabled: true, mode: "track", preampDb: 0 },
        runtime: { gainDb: 0.1, status: "clampedByPeak", reason: null },
      }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("normalization-status").textContent).toContain("削波"),
    );
  });

  it("reports a damaged preference as restored-off without losing the original data", async () => {
    renderWith(snapshot({ recoveryReason: "invalid normalization preference" }));
    await waitFor(() =>
      expect(screen.getByTestId("normalization-status").textContent).toContain("恢复为关闭"),
    );
  });

  it("read failure keeps a retry path", async () => {
    call.mockRejectedValueOnce(new Error("offline"));
    render(<NormalizationControls />);
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    call.mockResolvedValueOnce(snapshot());
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() =>
      expect(screen.getByTestId("normalization-status").textContent).toContain("已关闭"),
    );
  });
});
