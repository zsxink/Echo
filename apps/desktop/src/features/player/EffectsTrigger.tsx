import { useRef } from "react";
import { playerStore, useAudioEffects, usePlayerUi } from "../../player/playerStore";

export const effectsTriggerRef = { current: null as HTMLElement | null };

export function EffectsTrigger({ immersive = false }: { readonly immersive?: boolean }) {
  const ref = useRef<HTMLButtonElement>(null);
  const ui = usePlayerUi();
  const effects = useAudioEffects();
  const enabled = effects?.document.requestedEnabled === true;
  const confirmed = effects?.runtime.applied === "applied";
  const selected = effects?.document.selection;
  const name =
    enabled && confirmed
      ? selected?.kind === "draft"
        ? "自定义"
        : selected?.kind === "preset"
          ? effects?.presets.find((preset) => preset.id === selected.id)?.name
          : undefined
      : undefined;
  return (
    <button
      type="button"
      className={`effects-trigger ${immersive ? "effects-trigger-immersive" : "control"}${!immersive && name ? " has-name" : ""}${enabled ? " active" : ""}`}
      aria-label={`${ui.effectsOpen ? "隐藏" : "显示"}音效${name ? `，已生效：${name}` : ""}`}
      aria-expanded={ui.effectsOpen}
      aria-controls="audio-effects-panel"
      title={name ? `音效：${name}` : "音效"}
      ref={ref}
      onClick={() => {
        effectsTriggerRef.current = ref.current;
        playerStore.setEffectsOpen(!ui.effectsOpen);
      }}
    >
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        aria-hidden="true"
      >
        <path d="M5 3v5m0 4v9M12 3v10m0 4v4M19 3v2m0 4v12M2 10h6m1 5h6m1-8h6" />
      </svg>
      {immersive || name ? (
        <span className={!immersive && name ? "effects-trigger-name" : ""}>{name ?? "音效"}</span>
      ) : null}
    </button>
  );
}
