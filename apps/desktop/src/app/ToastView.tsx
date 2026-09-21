/**
 * Toast surface (prototype `.toast`).
 *
 * Sits above the player bar, centred, with the Echo mark, one line of text and
 * an optional action. The prototype never auto-dismisses a toast that carries an
 * action (`.toast-action` waits for the user), so a "撤销" affordance can never
 * vanish mid-click. Echo keeps that behaviour and adds one thing the prototype
 * has no data for: a real 10-second undo window (task 10.8), which the caller
 * requests explicitly through `autoDismissMs`.
 *
 * File name note: the external store lives in `toast.ts`; this presentational
 * component is `ToastView.tsx` so the two names differ by more than case (a
 * case-insensitive filesystem would otherwise resolve them to one file).
 */

import { useEffect, useRef, useState } from "react";

import { Icon } from "./Icon";
import { toastStore, useToast } from "./toast";

const DISMISS_MS = 3200;

export function ToastView() {
  const toast = useToast();
  // Pause belongs to one rendered toast. When an action replaces the current
  // toast (for example, 撤销 -> 已恢复), the component remains mounted, but
  // the new toast must start its own dismissal timer instead of inheriting the
  // old toast's hover/focus pause.
  const [pausedToastId, setPausedToastId] = useState<number | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const id = toast?.id ?? null;
  const paused = id !== null && pausedToastId === id;
  const autoDismissMs = toast?.autoDismissMs ?? null;
  const hasAction = Boolean(toast?.onAction);

  useEffect(() => {
    if (timer.current) clearTimeout(timer.current);
    if (id === null) return;
    // Hovering or focusing the toast holds it open, exactly as the prototype
    // does (`mouseenter` clears the timer).
    if (paused) return;
    // An action toast waits for the user unless the caller set a real window.
    const lifetime = autoDismissMs ?? (hasAction ? null : DISMISS_MS);
    if (lifetime === null) return;
    timer.current = setTimeout(() => toastStore.dismiss(), lifetime);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [id, paused, hasAction, autoDismissMs]);

  if (!toast) return null;

  return (
    <div
      className={`toast show${toast.error ? " is-error" : ""}`}
      role="status"
      aria-live="polite"
      data-testid="toast"
      onMouseEnter={() => setPausedToastId(id)}
      onMouseLeave={() => setPausedToastId(null)}
      onFocus={() => setPausedToastId(id)}
      onBlur={() => setPausedToastId(null)}
    >
      <span className="toast-icon" aria-hidden="true">
        <Icon name="note" />
      </span>
      <span className="toast-message" title={toast.message}>
        {toast.message}
      </span>
      {toast.onAction ? (
        <button
          type="button"
          className="toast-action"
          onClick={() => {
            const action = toast.onAction;
            toastStore.dismiss();
            action?.();
          }}
        >
          {toast.actionLabel ?? "撤销"}
        </button>
      ) : null}
    </div>
  );
}
