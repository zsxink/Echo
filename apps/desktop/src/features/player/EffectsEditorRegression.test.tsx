import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { EffectsSnapshotDto, EqCurve } from "../../ipc/ipc-types.generated";
import fixtureSource from "./__fixtures__/effects-snapshots.generated.json?raw";
import { EqualizerEditor, NEUTRAL_CURVE } from "./EqualizerEditor";
import { EffectsPresets } from "./EffectsPresets";

// These snapshots are emitted by the Rust IPC fixture generator. Installed
// activeBands and editableBands intentionally differ for an Applied spatial chain.
const fixtures: Record<string, EffectsSnapshotDto> = JSON.parse(fixtureSource);

function editor(effects: EffectsSnapshotDto, onEdit = vi.fn(), curve?: EqCurve) {
  return (
    <EqualizerEditor
      curve={
        curve ??
        (effects.document.retainedPayload.kind === "eq"
          ? effects.document.retainedPayload
          : NEUTRAL_CURVE)
      }
      activeBands={effects.editableBands ?? effects.runtime.activeBands}
      processingRate={effects.runtime.processingRate}
      responseRate={effects.responseRate ?? null}
      points={effects.responsePoints}
      reference={effects.referenceResponse}
      actualPreamp={effects.runtime.effectivePreampDb}
      spatial={effects.document.retainedPayload.kind === "spatial"}
      onEdit={onEdit}
      onSave={vi.fn().mockResolvedValue(undefined)}
      onReset={vi.fn()}
      canSave={false}
    />
  );
}

describe("backend snapshot equalizer regressions", () => {
  it.each(["pendingLowRate", "bypassedLowRate", "failedLowRate"])(
    "uses target EQ validity for the reference response in %s",
    (key) => {
      const effects = fixtures[key];
      expect(effects.runtime.processingRate).toBe(22050);
      expect(effects.referenceResponse).toBe(true);
      render(editor(effects));
      expect(screen.getByRole("slider", { name: "8000 Hz 增益" })).toBeEnabled();
      expect(screen.getByRole("slider", { name: "16000 Hz 增益" })).toBeDisabled();
      expect(screen.getByRole("img", { name: "参考均衡器响应" })).toBeInTheDocument();
      expect(screen.getByText("参考响应 · 22,050 Hz · 尚未确认生效")).toBeInTheDocument();
      expect(screen.getByText(/已确认实际前置增益：待确认/)).toBeInTheDocument();
    },
  );

  it("allows EQ editing to replace an Applied spatial chain", () => {
    const effects = fixtures.appliedSpatial;
    expect(effects.runtime.applied).toBe("applied");
    expect(effects.runtime.activeBands).toEqual(Array(10).fill(false));
    const onEdit = vi.fn();
    render(editor(effects, onEdit));
    const band = screen.getByRole("slider", { name: "1000 Hz 增益" });
    expect(band).toBeEnabled();
    fireEvent.keyDown(band, { key: "ArrowUp" });
    expect(onEdit).toHaveBeenCalledWith({
      ...NEUTRAL_CURVE,
      gainsDb: [0, 0, 0, 0, 0, 0.5, 0, 0, 0, 0],
    });
    expect(screen.getByText(/编辑均衡器将替换空间处理/)).toBeInTheDocument();
  });

  it.each(["manualProtected", "autoProtected"])(
    "keeps the requested slider in range and displays confirmed protection for %s",
    (key) => {
      const effects = fixtures[key];
      expect(effects.runtime.effectivePreampDb).toBeLessThan(-12);
      render(editor(effects));
      expect(screen.getByRole("slider", { name: "前置增益" })).toHaveValue("0");
      expect(screen.getByText("0.0 dB")).toBeInTheDocument();
      expect(
        screen.getByText(
          new RegExp(`已确认实际前置增益：${effects.runtime.effectivePreampDb?.toFixed(3)} dB`),
        ),
      ).toBeInTheDocument();
      if (key === "manualProtected") {
        expect(screen.getByText(/峰值保护已限制手动请求/)).toBeInTheDocument();
      }
    },
  );

  it("switches Auto and Manual without changing the curve or retained manual request", () => {
    const effects = fixtures.manualProtected;
    if (effects.document.retainedPayload.kind !== "eq") throw new Error("Expected EQ fixture");
    const curve = { ...effects.document.retainedPayload, requestedPreampDb: -6 };
    const onEdit = vi.fn();
    const view = render(editor(effects, onEdit));
    fireEvent.change(screen.getByRole("slider", { name: "前置增益" }), {
      target: { value: "-6" },
    });
    expect(onEdit).toHaveBeenLastCalledWith(curve);
    view.rerender(editor(effects, onEdit, curve));
    fireEvent.click(screen.getByRole("button", { name: "自动" }));
    const auto: EqCurve = { ...curve, preampMode: "auto" };
    expect(onEdit).toHaveBeenLastCalledWith(auto);
    view.rerender(editor(effects, onEdit, auto));
    expect(screen.getByRole("button", { name: "自动" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "手动" }));
    expect(onEdit).toHaveBeenLastCalledWith({ ...curve, preampMode: "manual" });
    expect(screen.getByRole("slider", { name: "前置增益" })).toHaveValue("-6");
  });

  it("labels a no-track response as a 48 kHz reference and leaves valid EQ edits available", () => {
    const effects = fixtures.noTrack;
    expect(effects.runtime.processingRate).toBeNull();
    render(editor(effects));
    expect(screen.getByText("参考响应 · 48,000 Hz · 尚未确认生效")).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /含实际前置增益/ })).not.toBeInTheDocument();
    expect(screen.getByRole("slider", { name: "16000 Hz 增益" })).toBeEnabled();
  });

  it("disables spatial presets for mono and explains the channel requirement", () => {
    const effects = fixtures.mono;
    const onSelect = vi.fn();
    render(<EffectsPresets effects={effects} onSelect={onSelect} onManage={vi.fn()} />);
    const spatial = screen.getByRole("button", { name: "空间感增强" });
    expect(spatial).toBeDisabled();
    expect(spatial).toHaveAccessibleDescription("单声道不可用，仅支持双声道");
    expect(screen.getByText("单声道不可用，仅支持双声道")).toBeVisible();
    fireEvent.click(spatial);
    expect(onSelect).not.toHaveBeenCalled();
  });
});
