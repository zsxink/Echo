/**
 * The mock's audio-effects command handlers.
 *
 * The equalizer document the app edits: selecting a builtin preset, editing
 * the curve, toggling the processor, resetting, and the preset-management
 * commands. The mock keeps the whole effects document in one in-memory
 * snapshot so a scenario can assert what each edit produced — the real
 * pipeline is Core's and is covered by the Rust suite.
 */
import type { EffectsSnapshotDto, EqCurve } from "../src/ipc/ipc-types.generated";
import type { Command, Handler } from "./mock-bridge-commands";

/** Build the audio-effects handler table for the mock backend. */
export function buildEffectsHandlers(): Partial<Record<Command, Handler>> {
  const rockCurve = {
    gainsDb: [3, 4, 2, 0, -1.5, -2, -0.5, 2, 3, 2],
    preampMode: "auto" as const,
    requestedPreampDb: 0,
  };
  let effectsSnapshot: EffectsSnapshotDto = {
    document: {
      schemaVersion: 1,
      registryVersion: 1,
      userPresets: [],
      selection: { kind: "none" },
      retainedPayload: { kind: "eq", ...rockCurve },
      draft: null,
      requestedEnabled: false,
    },
    runtime: {
      revision: 0,
      playbackEpoch: 0,
      persistenceStatus: "saved",
      applied: "bypassed",
      effectivePreampDb: 0,
      activeBands: [false, false, false, false, false, false, false, false, false, false],
      processingRate: null,
      channelLayout: null,
      reason: null,
    },
    presets: [
      {
        id: "builtin:rock",
        name: "摇滚",
        description: "突出低频和乐器存在感",
        payload: { kind: "eq", ...rockCurve },
        source: "builtin",
      },
    ],
    responsePoints: Array.from({ length: 201 }, (_, index) => ({
      frequencyHz: 20 * 1000 ** (index / 200),
      gainDb: 0,
    })),
    referenceResponse: false,
    safePreampDb: 0,
  };
  const reviseEffects = (
    document: EffectsSnapshotDto["document"],
    applied: EffectsSnapshotDto["runtime"]["applied"],
  ) => {
    effectsSnapshot = {
      ...effectsSnapshot,
      document,
      runtime: {
        ...effectsSnapshot.runtime,
        revision: effectsSnapshot.runtime.revision + 1,
        applied,
        persistenceStatus: "unsaved",
        reason: null,
      },
    };
    return effectsSnapshot;
  };

  return {
    get_audio_effects_snapshot: () => effectsSnapshot,
    select_audio_effects_preset: ({ id }) => {
      const preset = effectsSnapshot.presets.find((item) => item.id === String(id));
      if (!preset) throw new Error("unknown mock effects preset");
      return reviseEffects(
        {
          ...effectsSnapshot.document,
          selection: { kind: "preset", id: preset.id },
          retainedPayload: preset.payload,
          draft: null,
        },
        effectsSnapshot.document.requestedEnabled ? "pending" : "bypassed",
      );
    },
    edit_audio_equalizer: ({ curve }) =>
      reviseEffects(
        {
          ...effectsSnapshot.document,
          selection: { kind: "draft" },
          retainedPayload: {
            kind: "eq",
            ...(curve as EqCurve),
          },
          draft: curve as EqCurve,
        },
        effectsSnapshot.document.requestedEnabled ? "pending" : "bypassed",
      ),
    set_audio_effects_enabled: ({ enabled }) =>
      reviseEffects(
        { ...effectsSnapshot.document, requestedEnabled: Boolean(enabled) },
        enabled ? "pending" : "bypassed",
      ),
    reset_audio_effects: () =>
      reviseEffects(
        {
          ...effectsSnapshot.document,
          selection: { kind: "none" },
          requestedEnabled: false,
          draft: null,
          retainedPayload: {
            kind: "eq",
            gainsDb: Array(10).fill(0),
            preampMode: "auto",
            requestedPreampDb: 0,
          },
        },
        "bypassed",
      ),
    save_audio_effects_preset: () => effectsSnapshot,
    rename_audio_effects_preset: () => effectsSnapshot,
    delete_audio_effects_preset: () => effectsSnapshot,
    retry_audio_effects: () => effectsSnapshot,
  };
}
