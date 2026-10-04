import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EffectsSnapshotDto } from "../../ipc/ipc-types.generated";
import { bridge } from "../../bridge";
import { EMPTY_SNAPSHOT, playerStore } from "../../player/playerStore";
import { EffectsPanel } from "./EffectsPanel";
import { EffectsTrigger } from "./EffectsTrigger";
import { EffectsDialog } from "./EffectsDialog";

vi.mock("../../bridge", () => ({ bridge: { call: vi.fn() } }));
const call = vi.mocked(bridge.call);
let revision = 1000;
function snapshot(): EffectsSnapshotDto {
  const curve = { gainsDb: Array(10).fill(0), preampMode: "auto" as const, requestedPreampDb: 0 };
  return {
    document: {
      schemaVersion: 1,
      registryVersion: 1,
      userPresets: [],
      selection: { kind: "draft" },
      retainedPayload: { kind: "eq", ...curve },
      draft: curve,
      requestedEnabled: true,
    },
    runtime: {
      revision: ++revision,
      playbackEpoch: 1,
      persistenceStatus: "saved",
      applied: "pending",
      effectivePreampDb: -2,
      activeBands: [true, true, true, true, true, true, true, true, true, false],
      processingRate: 22050,
      channelLayout: "stereo",
      reason: null,
    },
    presets: [
      {
        id: "builtin:neutral",
        name: "中性",
        description: "平直频响",
        source: "builtin",
        payload: { kind: "eq", ...curve },
      },
    ],
    responsePoints: [
      { frequencyHz: 20, gainDb: -2 },
      { frequencyHz: 1000, gainDb: -2 },
      { frequencyHz: 10000, gainDb: -2 },
    ],
    referenceResponse: false,
    safePreampDb: -2,
  };
}
function openPanel(effects: EffectsSnapshotDto) {
  playerStore.publishEffects(effects);
  call.mockResolvedValue(effects);
  render(
    <>
      <EffectsTrigger />
      <EffectsPanel />
      <button type="button">外部目标</button>
    </>,
  );
  fireEvent.click(screen.getByRole("button", { name: /^显示音效/ }));
}

beforeEach(() => {
  call.mockReset();
  playerStore.setEffectsOpen(false);
  playerStore.setQueueOpen(false);
  playerStore.setImmersiveOpen(false);
  playerStore.setEffectsTab("presets");
  playerStore.publish(EMPTY_SNAPSHOT);
});

describe("audio effects shared player state", () => {
  it("rejects older revisions and stale playback epochs", () => {
    const current = snapshot();
    playerStore.publishEffects(current);
    playerStore.publishEffects({
      ...current,
      runtime: { ...current.runtime, revision: current.runtime.revision - 1, applied: "applied" },
    });
    expect(playerStore.getEffects()?.runtime.applied).toBe("pending");
    playerStore.publishEffects({
      ...current,
      runtime: { ...current.runtime, playbackEpoch: 0, applied: "applied" },
    });
    expect(playerStore.getEffects()?.runtime.applied).toBe("pending");
  });
  it("projects an optional snapshot into the unique effects store", () => {
    const effects = snapshot();
    playerStore.publish({ ...EMPTY_SNAPSHOT, effects });
    expect(playerStore.getEffects()).toEqual(effects);
  });
  it("makes the queue and effects panel mutually exclusive without losing the draft", () => {
    const effects = snapshot();
    playerStore.publishEffects(effects);
    playerStore.setQueueOpen(true);
    playerStore.setEffectsOpen(true);
    expect(playerStore.getUi().queueOpen).toBe(false);
    playerStore.setQueueOpen(true);
    expect(playerStore.getUi().effectsOpen).toBe(false);
    expect(playerStore.getEffects()?.document.draft).toEqual(effects.document.draft);
  });
  it("shows the enabled selection while pending and marks it applied after confirmation", async () => {
    const effects = snapshot();
    openPanel(effects);
    expect(screen.getByRole("button", { name: "隐藏音效，已启用，待应用：自定义" })).toBeInTheDocument();
    await act(async () =>
      playerStore.publishEffects({
        ...effects,
        runtime: { ...effects.runtime, applied: "applied" },
      }),
    );
    expect(screen.getByRole("button", { name: "隐藏音效，已生效：自定义" })).toBeInTheDocument();
    act(() =>
      playerStore.publishEffects({
        ...effects,
        runtime: { ...effects.runtime, applied: "unavailable" },
      }),
    );
    expect(screen.getByText(/当前音频环境不可用/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试" })).toBeInTheDocument();
  });
  it("retries snapshot read failures with GET only until the read succeeds", async () => {
    const effects = snapshot();
    playerStore.publishEffects(effects);
    call
      .mockRejectedValueOnce(new Error("read failed"))
      .mockRejectedValueOnce(new Error("read failed again"))
      .mockResolvedValueOnce(effects);
    render(
      <>
        <EffectsTrigger />
        <EffectsPanel />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: /^显示音效/ }));

    await waitFor(() => expect(screen.getByText("读取音效失败，请重试。")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(call).toHaveBeenCalledTimes(2));
    expect(screen.getByText("读取音效失败，请重试。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(call).toHaveBeenCalledTimes(3));
    await waitFor(() =>
      expect(screen.queryByText("读取音效失败，请重试。")).not.toBeInTheDocument(),
    );
    expect(call.mock.calls.map(([name]) => name)).toEqual([
      "get_audio_effects_snapshot",
      "get_audio_effects_snapshot",
      "get_audio_effects_snapshot",
    ]);
  });
  it("shows a recovery repair message without exposing its reason", async () => {
    const base = snapshot();
    const effects: EffectsSnapshotDto = {
      ...base,
      runtime: { ...base.runtime, persistenceStatus: "failed" },
      recoveryReason: "private storage detail",
    };
    openPanel(effects);

    expect(screen.getByText("音效恢复失败 · 原数据已保留，请重试修复。")).toBeInTheDocument();
    expect(screen.queryByText("private storage detail")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重试修复" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("retry_audio_effects"));
  });
});

describe("nonmodal effects overlay", () => {
  it("closes once on a repeated trigger gesture", () => {
    openPanel(snapshot());
    const trigger = screen.getByRole("button", { name: /^隐藏音效/ });
    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    expect(screen.queryByTestId("effects-panel")).not.toBeInTheDocument();
  });
  it("allows Tab focus to leave and preserves the destination", () => {
    openPanel(snapshot());
    const target = screen.getByRole("button", { name: "外部目标" });
    act(() => target.focus());
    expect(screen.queryByTestId("effects-panel")).not.toBeInTheDocument();
    expect(target).toHaveFocus();
  });
  it("keeps outside-click focus and closes Escape at a single level", () => {
    openPanel(snapshot());
    const target = screen.getByRole("button", { name: "外部目标" });
    fireEvent.pointerDown(target);
    act(() => target.focus());
    expect(target).toHaveFocus();
    expect(screen.queryByTestId("effects-panel")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^显示音效/ }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("button", { name: /^显示音效/ })).toHaveFocus();
  });
  it("uses linked tabs and left/right keys without changing playback", () => {
    openPanel(snapshot());
    const tab = screen.getByRole("tab", { name: "音效" });
    fireEvent.keyDown(tab, { key: "ArrowRight" });
    const equalizer = screen.getByRole("tab", { name: "均衡器" });
    expect(equalizer).toHaveFocus();
    expect(equalizer).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", equalizer.id);
    expect(call).not.toHaveBeenCalledWith("player_control", expect.anything());
  });
});

describe("equalizer and management", () => {
  it("keeps disabled bands and supports 0.5 dB arrows and limits", async () => {
    const effects = snapshot();
    openPanel(effects);
    fireEvent.click(screen.getByRole("tab", { name: "均衡器" }));
    expect(screen.getByRole("slider", { name: "16000 Hz 增益" })).toBeDisabled();
    const slider = screen.getByRole("slider", { name: "1000 Hz 增益" });
    fireEvent.keyDown(slider, { key: "ArrowUp" });
    expect(call).toHaveBeenCalledWith("edit_audio_equalizer", {
      curve: expect.objectContaining({ gainsDb: [0, 0, 0, 0, 0, 0.5, 0, 0, 0, 0] }),
    });
    fireEvent.keyDown(slider, { key: "Home" });
    expect(call).toHaveBeenCalledWith("edit_audio_equalizer", {
      curve: expect.objectContaining({ gainsDb: [0, 0, 0, 0, 0, -12, 0, 0, 0, 0] }),
    });
    fireEvent.keyDown(slider, { key: "End" });
    expect(call).toHaveBeenCalledWith("edit_audio_equalizer", {
      curve: expect.objectContaining({ gainsDb: [0, 0, 0, 0, 0, 12, 0, 0, 0, 0] }),
    });
    await act(async () => undefined);
    expect(screen.getByRole("slider", { name: "前置增益" })).toHaveAttribute(
      "aria-valuetext",
      "-2 dB，自动保护",
    );
    expect(screen.getByRole("img", { name: /含实际前置增益/ })).toBeInTheDocument();
  });
  it("suppresses spatial save and displays the replacement explanation", () => {
    const base = snapshot();
    const effects: EffectsSnapshotDto = {
      ...base,
      document: {
        ...base.document,
        retainedPayload: { kind: "spatial", width: 1.25, mix: 1 },
        draft: null,
        selection: { kind: "preset", id: "builtin:spatial" },
      },
      responsePoints: [],
    };
    openPanel(effects);
    fireEvent.click(screen.getByRole("tab", { name: "均衡器" }));
    expect(screen.getByText(/编辑均衡器将替换空间处理/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存音效" })).toBeDisabled();
  });
  it("shows unsaved feedback and preserves the inline preset name after a failed save", async () => {
    const base = snapshot();
    const effects: EffectsSnapshotDto = {
      ...base,
      runtime: { ...base.runtime, persistenceStatus: "failed" },
    };
    openPanel(effects);
    expect(screen.getByText(/尚未保存到本机/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "均衡器" }));
    fireEvent.change(screen.getByLabelText("自定义音效名称"), {
      target: { value: "我的低音" },
    });
    call.mockRejectedValueOnce(new Error("disk full"));
    fireEvent.click(screen.getByRole("button", { name: "保存音效" }));
    await waitFor(() => expect(screen.getByText(/操作失败，请求和曲线已保留/)).toBeInTheDocument());
    expect(screen.getByLabelText("自定义音效名称")).toHaveValue("我的低音");
    expect(screen.getByTestId("effects-panel")).toBeInTheDocument();
  });
  it("does not close a name dialog before the backend acknowledges a transaction", async () => {
    let finish: (() => void) | undefined;
    const submit = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    const close = vi.fn();
    render(<EffectsDialog action={{ kind: "save" }} onClose={close} onSubmit={submit} />);
    fireEvent.change(screen.getByLabelText("曲线名称（1–40 个字符）"), {
      target: { value: "关闭草稿" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "正在保存…" })).toBeDisabled();
    await act(async () => finish?.());
    expect(close).toHaveBeenCalledOnce();
  });
});
