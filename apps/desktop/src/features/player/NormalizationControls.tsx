/**
 * Independent loudness-normalization controls.
 * Not a preset: the switch, mode and preamp are a separate preference owned by
 * NormalizationService. Copy stays with the observable facts (existing tags,
 * sample-peak protection) and never claims measured loudness or true peak.
 */
import { useEffect, useState } from "react";
import { bridge } from "../../bridge";
import type {
  NormalizationPreference,
  NormalizationSnapshotDto,
  NormalizationStatus,
} from "../../ipc/ipc-types.generated";

const STATUS_TEXT: Record<NormalizationStatus, string> = {
  disabled: "已关闭 · 播放保持原始音量",
  noTags: "当前曲目无可用响度标签 · 不调整音量",
  applied: "已按文件内标签调整音量",
  clampedByPeak: "为避免削波已限制提升",
  limitedByCap: "标签无峰值信息，提升已限制并由峰值保护承接",
};

function statusText(state: NormalizationSnapshotDto): string {
  if (state.recoveryReason) {
    return "偏好读取失败 · 已恢复为关闭，原数据已保留";
  }
  const text = STATUS_TEXT[state.runtime.status];
  if (state.runtime.reason) return `${text} · ${state.runtime.reason}`;
  if (state.runtime.status === "applied" || state.runtime.status === "clampedByPeak") {
    return `${text} · ${state.runtime.gainDb.toFixed(1)} dB`;
  }
  return text;
}

export function NormalizationControls() {
  const [state, setState] = useState<NormalizationSnapshotDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void bridge
      .call("get_loudness_normalization")
      .then((snapshot) => {
        if (active) {
          setState(snapshot);
          setError(null);
        }
      })
      .catch(() => {
        if (active) setError("读取音量归一化偏好失败，请重试。");
      });
    return () => {
      active = false;
    };
  }, []);

  async function update(patch: Partial<NormalizationPreference>) {
    if (!state) return;
    const preference: NormalizationPreference = { ...state.preference, ...patch };
    try {
      const next = await bridge.call("set_loudness_normalization", { preference });
      setState(next);
      setError(null);
    } catch {
      setError("未能保存偏好，请重试。");
    }
  }

  function retry() {
    setError(null);
    void bridge
      .call("get_loudness_normalization")
      .then(setState)
      .catch(() => setError("读取音量归一化偏好失败，请重试。"));
  }

  return (
    <section
      className="normalization-controls"
      data-testid="normalization-controls"
      aria-label="音量归一化"
    >
      <div className="normalization-head">
        <h3>音量归一化</h3>
        <label className="effect-toggle">
          <input
            type="checkbox"
            role="switch"
            aria-label="启用音量归一化"
            checked={state?.preference.enabled ?? false}
            disabled={!state}
            onChange={(event) => void update({ enabled: event.target.checked })}
          />
          启用
        </label>
      </div>
      <p className="normalization-note">
        读取文件中已有的 ReplayGain / R128 标签调整曲目音量；采用 sample-peak 保护，不测量节目响度。
      </p>
      {state ? (
        <>
          <div className="normalization-row">
            <label htmlFor="normalization-mode">依据</label>
            <select
              id="normalization-mode"
              aria-label="归一化依据"
              value={state.preference.mode}
              onChange={(event) =>
                void update({
                  mode: event.target.value === "album" ? "album" : "track",
                })
              }
            >
              <option value="track">单曲</option>
              <option value="album">专辑</option>
            </select>
            <label htmlFor="normalization-preamp">
              前置增益 {state.preference.preampDb.toFixed(1)} dB
            </label>
            <input
              id="normalization-preamp"
              type="range"
              min={-12}
              max={12}
              step={0.5}
              value={state.preference.preampDb}
              onChange={(event) => void update({ preampDb: Number(event.target.value) })}
            />
          </div>
          <p className="normalization-status" role="status" data-testid="normalization-status">
            {statusText(state)}
          </p>
        </>
      ) : (
        <p className="normalization-status">正在读取偏好…</p>
      )}
      {error ? (
        <p className="normalization-error" role="alert">
          {error}
          <button type="button" className="effects-retry" onClick={retry}>
            重试
          </button>
        </p>
      ) : null}
    </section>
  );
}
