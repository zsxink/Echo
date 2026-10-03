/**
 * THESIS: select a listening preset, then edit its exact EQ without leaving playback.
 * OWN-WORLD: Echo's white surfaces, theme accent, compact buttons and semantic text.
 * STORY: requested settings stay visible while native confirmation and persistence remain explicit.
 * FIRST VIEWPORT: fixed switch and two tabs above one independently scrolling content area.
 * FORM: a nonmodal player popover; chart, frequency labels, vertical ranges and preamp occupy separate rows.
 */
import { useEffect, useRef, useState } from "react";
import { bridge } from "../../bridge";
import { OverlayTier, useOverlay } from "../../app/overlays";
import { playerStore, useAudioEffects, usePlayerUi } from "../../player/playerStore";
import type { EffectsSnapshotDto, EqCurve } from "../../ipc/ipc-types.generated";
import { EffectsDialog, type EffectsDialogAction } from "./EffectsDialog";
import { EffectsPresets } from "./EffectsPresets";
import { EqualizerEditor, NEUTRAL_CURVE } from "./EqualizerEditor";
import { effectsTriggerRef } from "./EffectsTrigger";
import "../../styles/effects.css";

export function effectsStatus(effects: EffectsSnapshotDto): string {
  const { runtime, document } = effects;
  if (runtime.applied === "applied" && document.requestedEnabled) {
    const selection = document.selection;
    const name =
      selection.kind === "draft"
        ? "自定义"
        : selection.kind === "preset"
          ? effects.presets.find((preset) => preset.id === selection.id)?.name
          : null;
    return name ? `${name}已生效` : "音效已生效";
  }
  if (runtime.applied === "pending") return "待应用 · 正在等待音频链确认";
  if (runtime.applied === "failed") return "应用失败 · 请求已保留，可重试";
  if (runtime.applied === "unavailable") return "当前音频环境不可用 · 请求已保留";
  return "音效已关闭";
}

export function EffectsPanel() {
  const ui = usePlayerUi();
  const effects = useAudioEffects();
  const container = useRef<HTMLElement>(null);
  const [dialog, setDialog] = useState<EffectsDialogAction | null>(null);
  const [error, setError] = useState<{ message: string; source: "read" | "operation" } | null>(
    null,
  );
  const [announcement, setAnnouncement] = useState("");
  const [editing, setEditing] = useState<EqCurve | null>(null);
  const editSequence = useRef(0);
  useOverlay({
    tier: OverlayTier.Menu,
    containerRef: container,
    triggerRef: effectsTriggerRef,
    enabled: ui.effectsOpen,
    onClose: () => playerStore.setEffectsOpen(false),
    nonModal: true,
  });

  useEffect(() => {
    if (!ui.effectsOpen) return;
    let active = true;
    void bridge
      .call("get_audio_effects_snapshot")
      .then((snapshot) => {
        playerStore.publishEffects(snapshot);
        if (active) setError(null);
      })
      .catch(() => {
        if (active) setError({ message: "读取音效失败，请重试。", source: "read" });
      });
    return () => {
      active = false;
    };
  }, [ui.effectsOpen]);

  async function retryReadSnapshot() {
    try {
      const snapshot = await bridge.call("get_audio_effects_snapshot");
      playerStore.publishEffects(snapshot);
      setError(null);
    } catch {
      setError({ message: "读取音效失败，请重试。", source: "read" });
    }
  }

  const status = effects ? effectsStatus(effects) : "正在读取音效…";
  useEffect(() => {
    // Slider changes aggregate into one polite announcement after the interaction settles.
    const timer = window.setTimeout(() => setAnnouncement(status), 350);
    return () => window.clearTimeout(timer);
  }, [status, effects?.runtime.revision]);

  async function apply(request: Promise<EffectsSnapshotDto>) {
    try {
      const snapshot = await request;
      playerStore.publishEffects(snapshot);
      setError(null);
      return snapshot;
    } catch (cause) {
      setError({ message: "操作失败，请求和曲线已保留，请重试。", source: "operation" });
      throw cause;
    }
  }
  function command(request: Promise<EffectsSnapshotDto>) {
    void apply(request).catch(() => undefined);
  }
  function edit(curve: EqCurve) {
    const sequence = ++editSequence.current;
    setEditing(curve);
    void apply(bridge.call("edit_audio_equalizer", { curve }))
      .then(() => {
        if (sequence === editSequence.current) setEditing(null);
      })
      .catch(() => undefined);
  }
  function replace(request: Promise<EffectsSnapshotDto>) {
    ++editSequence.current;
    setEditing(null);
    command(request);
  }
  if (!ui.effectsOpen) return null;
  const canEnable =
    effects && (effects.document.selection.kind !== "none" || effects.document.draft !== null);
  const spatial = effects?.document.retainedPayload.kind === "spatial";
  const curve =
    editing ??
    (effects?.document.retainedPayload.kind === "eq"
      ? effects.document.retainedPayload
      : NEUTRAL_CURVE);
  return (
    <>
      <section
        id="audio-effects-panel"
        data-testid="effects-panel"
        className="effects-dialog"
        data-view={ui.effectsTab === "presets" ? "picker" : "eq"}
        role="dialog"
        aria-label="音效与均衡器"
        aria-modal="false"
        ref={container}
      >
        <div className="effects-panel">
          <header className="effects-head">
            <div
              className="effects-tabs"
              role="tablist"
              aria-label="音效设置"
              onKeyDown={(event) => {
                if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
                event.preventDefault();
                const next = ui.effectsTab === "presets" ? "equalizer" : "presets";
                playerStore.setEffectsTab(next);
                document.getElementById(`effects-tab-${next}`)?.focus();
              }}
            >
              {(["presets", "equalizer"] as const).map((tab) => (
                <button
                  key={tab}
                  type="button"
                  role="tab"
                  id={`effects-tab-${tab}`}
                  className="effects-tab"
                  aria-selected={ui.effectsTab === tab}
                  aria-controls={`effects-content-${tab}`}
                  tabIndex={ui.effectsTab === tab ? 0 : -1}
                  onClick={() => playerStore.setEffectsTab(tab)}
                >
                  {tab === "presets" ? "音效" : "均衡器"}
                </button>
              ))}
            </div>
            <label className={`effect-toggle${!canEnable ? " is-disabled" : ""}`}>
              <input
                type="checkbox"
                role="switch"
                checked={effects?.document.requestedEnabled ?? false}
                disabled={!canEnable}
                onChange={(event) =>
                  replace(
                    bridge.call("set_audio_effects_enabled", { enabled: event.target.checked }),
                  )
                }
                aria-label="启用音效处理"
                aria-describedby={!canEnable ? "effects-enable-reason" : undefined}
              />
              启用
            </label>
            <button
              className="settings-close"
              type="button"
              aria-label="关闭音效面板"
              onClick={() => playerStore.setEffectsOpen(false)}
            >
              ×
            </button>
          </header>
          <div className="effects-body">
            {error ||
            effects?.runtime.applied === "failed" ||
            effects?.runtime.applied === "unavailable" ||
            effects?.runtime.persistenceStatus === "failed" ||
            effects?.recoveryReason ? (
              <div className="effects-feedback" role="status">
                {!canEnable ? (
                  <span id="effects-enable-reason">选择预设或编辑均衡器后可启用。</span>
                ) : null}
                <span>
                  {error?.message ||
                    (effects?.recoveryReason
                      ? "音效恢复失败 · 原数据已保留，请重试修复。"
                      : effects?.runtime.persistenceStatus === "failed"
                        ? "尚未保存到本机 · 内存草稿已保留"
                        : status)}
                </span>
                {error ||
                effects?.runtime.applied === "failed" ||
                effects?.runtime.applied === "unavailable" ||
                effects?.runtime.persistenceStatus === "failed" ||
                effects?.recoveryReason ? (
                  <button
                    className="effects-retry"
                    type="button"
                    onClick={() =>
                      error?.source === "read"
                        ? void retryReadSnapshot()
                        : command(bridge.call("retry_audio_effects"))
                    }
                  >
                    {effects?.recoveryReason && error?.source !== "read" ? "重试修复" : "重试"}
                  </button>
                ) : null}
              </div>
            ) : null}
            {!effects ? (
              <p>正在读取音效…</p>
            ) : ui.effectsTab === "presets" ? (
              <div
                className="effects-sidebar"
                role="tabpanel"
                id="effects-content-presets"
                aria-labelledby="effects-tab-presets"
              >
                <EffectsPresets
                  effects={effects}
                  onSelect={(id) => replace(bridge.call("select_audio_effects_preset", { id }))}
                  onManage={setDialog}
                />
              </div>
            ) : (
              <div
                className="effects-main"
                role="tabpanel"
                id="effects-content-equalizer"
                aria-labelledby="effects-tab-equalizer"
              >
                <EqualizerEditor
                  curve={curve}
                  activeBands={effects.runtime.activeBands}
                  processingRate={effects.runtime.processingRate}
                  points={effects.responsePoints}
                  reference={effects.referenceResponse}
                  actualPreamp={effects.runtime.effectivePreampDb}
                  spatial={Boolean(spatial) && editing === null}
                  onEdit={edit}
                  onSave={(name) =>
                    apply(bridge.call("save_audio_effects_preset", { name })).then(() => {
                      setAnnouncement("曲线已保存");
                    })
                  }
                  onReset={() => replace(bridge.call("reset_audio_effects"))}
                  canSave={
                    effects.document.selection.kind === "draft" &&
                    effects.document.userPresets.length < 50 &&
                    editing === null
                  }
                  saveNotice={
                    effects.runtime.persistenceStatus === "failed"
                      ? "尚未保存到本机 · 内存草稿已保留"
                      : effects.runtime.persistenceStatus === "unsaved"
                        ? "正在保存到本机…"
                        : undefined
                  }
                />
              </div>
            )}
          </div>
          <span className="sr-only" aria-live="polite">
            {announcement}
          </span>
        </div>
      </section>
      {dialog ? (
        <EffectsDialog
          action={dialog}
          onClose={() => setDialog(null)}
          onSubmit={async (name) => {
            const snapshot = await apply(
              dialog.kind === "save"
                ? bridge.call("save_audio_effects_preset", { name })
                : dialog.kind === "rename"
                  ? bridge.call("rename_audio_effects_preset", { id: dialog.id, name })
                  : bridge.call("delete_audio_effects_preset", { id: dialog.id }),
            );
            setAnnouncement(dialog.kind === "delete" ? "曲线已删除" : "曲线已保存");
            playerStore.publishEffects(snapshot);
          }}
        />
      ) : null}
    </>
  );
}
