import { useState, type KeyboardEvent } from "react";
import type { EqCurve } from "../../ipc/ipc-types.generated";

const FREQUENCIES = [31.25, 62.5, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
const LABELS = ["31.25", "62.5", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
export const NEUTRAL_CURVE: EqCurve = {
  gainsDb: Array(10).fill(0),
  preampMode: "auto",
  requestedPreampDb: 0,
};

export interface ResponsePoint {
  readonly frequencyHz: number;
  readonly gainDb: number;
}

function rangeKey(
  event: KeyboardEvent<HTMLInputElement>,
  value: number,
  minimum: number,
  maximum: number,
  update: (value: number) => void,
) {
  const next =
    event.key === "ArrowUp"
      ? value + 0.5
      : event.key === "ArrowDown"
        ? value - 0.5
        : event.key === "Home"
          ? minimum
          : event.key === "End"
            ? maximum
            : null;
  if (next === null) return;
  event.preventDefault();
  update(Math.max(minimum, Math.min(maximum, next)));
}

export function EqualizerEditor({
  curve,
  activeBands,
  processingRate,
  points,
  reference,
  actualPreamp,
  spatial,
  onEdit,
  onSave,
  onReset,
  canSave,
  saveNotice,
}: {
  readonly curve: EqCurve;
  readonly activeBands: readonly boolean[];
  readonly processingRate: number | null;
  readonly points: readonly ResponsePoint[];
  readonly reference: boolean;
  readonly actualPreamp: number | null;
  readonly spatial: boolean;
  readonly onEdit: (curve: EqCurve) => void;
  readonly onSave: (name: string) => Promise<void>;
  readonly onReset: () => void;
  readonly canSave: boolean;
  readonly saveNotice?: string;
}) {
  const displayCurve = spatial ? NEUTRAL_CURVE : curve;
  function band(index: number, value: number) {
    const gainsDb = [...displayCurve.gainsDb];
    gainsDb[index] = value;
    onEdit({ ...displayCurve, gainsDb });
  }
  // Coordinates only: the backend supplies the composite digital filter response.
  const finitePoints = points.filter(
    (point) => point.frequencyHz > 0 && Number.isFinite(point.gainDb),
  );
  const minimum = Math.min(-18, ...finitePoints.map((point) => point.gainDb));
  const maximum = Math.max(12, ...finitePoints.map((point) => point.gainDb));
  const lastFrequency = Math.max(20000, ...finitePoints.map((point) => point.frequencyHz));
  const y = (gain: number) => 15 + ((maximum - gain) / (maximum - minimum)) * 70;
  const path = finitePoints
    .map(
      (point, index) =>
        `${index ? "L" : "M"}${((Math.log(Math.max(20, point.frequencyHz) / 20) / Math.log(lastFrequency / 20)) * 520).toFixed(2)},${y(point.gainDb).toFixed(2)}`,
    )
    .join(" ");
  const [presetName, setPresetName] = useState("");
  const protectedGain =
    actualPreamp !== null &&
    (displayCurve.preampMode === "auto" || actualPreamp < displayCurve.requestedPreampDb);
  return (
    <div className="equalizer-editor">
      {spatial ? (
        <p className="effects-note">当前为空间感增强。编辑均衡器将替换空间处理。</p>
      ) : null}
      <figure className="effect-preview">
        {spatial ? (
          <p className="effects-note">空间处理不以均衡器频响表示。</p>
        ) : points.length ? (
          <svg viewBox="0 0 520 100" role="img" aria-label="含实际前置增益的均衡器响应曲线">
            <path d="M0 15H520M0 50H520M0 85H520" className="gridline" />
            <path d={path} className="response-line" />
          </svg>
        ) : (
          <p className="effects-note">等待处理环境与响应数据。</p>
        )}
        {!spatial ? (
          <div className="effect-preview-labels" aria-hidden="true">
            <span>低频</span>
            <span>中频</span>
            <span>高频</span>
          </div>
        ) : null}
      </figure>
      <div className="eq-bands" role="group" aria-label="十段均衡器">
        {FREQUENCIES.map((frequency, index) => {
          const active = reference || activeBands[index];
          const disabledReason = processingRate
            ? `${frequency} Hz 超出当前处理采样率的有效范围，参数已保留`
            : "等待实际处理环境确认";
          return (
            <div className="eq-band" key={frequency}>
              <label htmlFor={`eq-band-${index}`}>{LABELS[index]}</label>
              <input
                id={`eq-band-${index}`}
                type="range"
                min={-12}
                max={12}
                step={0.5}
                value={displayCurve.gainsDb[index]}
                disabled={!active}
                aria-label={`${frequency} Hz 增益`}
                aria-valuetext={`${displayCurve.gainsDb[index]} dB`}
                aria-describedby={!active ? `eq-reason-${index}` : undefined}
                onChange={(event) => band(index, Number(event.target.value))}
                onKeyDown={(event) =>
                  rangeKey(event, displayCurve.gainsDb[index], -12, 12, (value) =>
                    band(index, value),
                  )
                }
              />
              <output htmlFor={`eq-band-${index}`}>{displayCurve.gainsDb[index].toFixed(1)}</output>
              {!active ? (
                <span id={`eq-reason-${index}`} className="sr-only">
                  {disabledReason}
                </span>
              ) : null}
            </div>
          );
        })}
      </div>
      <fieldset className="eq-preamp">
        <label className="eq-preamp-label" htmlFor="eq-preamp">
          前置增益
        </label>
        <output className="eq-preamp-output" htmlFor="eq-preamp">
          {(displayCurve.preampMode === "auto"
            ? actualPreamp
            : displayCurve.requestedPreampDb
          )?.toFixed(1) ?? "—"}{" "}
          dB
        </output>
        <input
          id="eq-preamp"
          type="range"
          min={-12}
          max={0}
          step={0.5}
          value={
            displayCurve.preampMode === "auto"
              ? (actualPreamp ?? 0)
              : displayCurve.requestedPreampDb
          }
          aria-label="前置增益"
          aria-valuetext={`${displayCurve.preampMode === "auto" ? (actualPreamp ?? 0) : displayCurve.requestedPreampDb} dB${displayCurve.preampMode === "auto" ? "，自动保护" : "，手动"}`}
          onChange={(event) =>
            onEdit({
              ...displayCurve,
              preampMode: "manual",
              requestedPreampDb: Number(event.target.value),
            })
          }
        />
        <button className="link-button eq-reset" type="button" onClick={onReset}>
          重置
        </button>
      </fieldset>
      <div className="save-preset">
        <input
          type="text"
          maxLength={40}
          aria-label="自定义音效名称"
          placeholder="为当前曲线命名"
          value={presetName}
          onChange={(event) => setPresetName(event.target.value)}
        />
        <button
          type="button"
          disabled={!canSave || spatial || !presetName.trim()}
          onClick={() => {
            void onSave(presetName.trim())
              .then(() => setPresetName(""))
              .catch(() => undefined);
          }}
        >
          保存音效
        </button>
      </div>
      <p className="effects-note" role="status" aria-live="polite">
        {saveNotice ??
          (reference
            ? "选择预设后可直接试听，也可调整频段再保存。"
            : `选择预设后可直接试听，也可调整频段再保存。${protectedGain ? "自动保护已为整条链保留峰值余量。" : ""}`)}
      </p>
    </div>
  );
}
