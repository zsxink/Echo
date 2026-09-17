import { useEffect, useRef, useState } from "react";

/**
 * Auto-follow state for the lyrics column (spec task 11.6): while following, the
 * column scrolls itself to keep the current line centred. A manual scroll
 * (read from the *input* — see `ImmersiveBody.onReaderScroll`, never the scroll
 * event) pauses the follow for 5 seconds, after which it resumes on its own;
 * 回到当前行 resumes it immediately.
 *
 * Isolated from the body so the timer-and-flag lifecycle is the only thing this
 * hook owns and can be reasoned about (and tested) without the surface markup.
 */
export interface LyricsFollow {
  readonly autoScroll: boolean;
  readonly pauseFollow: () => void;
  readonly resumeFollow: () => void;
}

export function useLyricsFollow(): LyricsFollow {
  const [autoScroll, setAutoScroll] = useState(true);
  const autoScrollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
    },
    [],
  );
  const pauseFollow = () => {
    setAutoScroll(false);
    if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
    // Spec (task 11.6): 停止操作 5 秒后恢复跟随.
    autoScrollTimer.current = setTimeout(() => setAutoScroll(true), 5000);
  };
  const resumeFollow = () => {
    setAutoScroll(true);
    if (autoScrollTimer.current) clearTimeout(autoScrollTimer.current);
  };
  return { autoScroll, pauseFollow, resumeFollow };
}
