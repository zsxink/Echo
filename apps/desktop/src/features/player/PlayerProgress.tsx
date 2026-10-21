/** Playback progress input; frame-rate updates stay isolated from the full bar. */

import { bridge } from "../../bridge";
import { useSmoothPosition } from "../../player/playerStore";
import { formatDuration } from "../library";

export function PlayerProgress({
  duration,
  disabled,
}: {
  readonly duration: number;
  readonly disabled: boolean;
}) {
  const position = useSmoothPosition() ?? 0;
  const progress = duration > 0 ? Math.max(0, Math.min(1, position / duration)) : 0;

  return (
    <div className="progress-line" id="progress-line">
      <span
        className="progress-fill"
        aria-hidden="true"
        style={{ transform: `scaleX(${progress})` }}
      />
      <input
        className="progress"
        id="progress"
        type="range"
        role="slider"
        aria-label="播放进度"
        aria-valuetext={`${formatDuration(position)} / ${formatDuration(duration)}`}
        min={0}
        max={duration || 0}
        step="any"
        value={position}
        disabled={disabled || duration <= 0}
        onKeyDown={(event) => {
          const delta =
            event.key === "ArrowRight" || event.key === "ArrowUp"
              ? 5
              : event.key === "ArrowLeft" || event.key === "ArrowDown"
                ? -5
                : null;
          const target =
            event.key === "Home"
              ? 0
              : event.key === "End"
                ? duration
                : delta !== null
                  ? Math.max(0, Math.min(duration, position + delta))
                  : null;
          if (target === null) return;
          event.preventDefault();
          bridge.fireAndForget("seek", { position: target });
        }}
        onChange={(event) => bridge.fireAndForget("seek", { position: Number(event.target.value) })}
      />
    </div>
  );
}
