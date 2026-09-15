/**
 * Toast store (prototype `.toast` surface).
 *
 * Non-blocking, single-line operation feedback with an optional action (the
 * prototype uses it for "已加入队列" style confirmations and for the 10-second
 * "撤销" affordance). One toast at a time — a newer message replaces the
 * current one, which is exactly what the prototype does.
 *
 * The store is deliberately tiny and external (like `playerStore`) so any
 * component can report an outcome without prop drilling.
 */

import { useSyncExternalStore } from "react";

export interface ToastRequest {
  readonly message: string;
  /** An error variant: the prototype paints a danger border. */
  readonly error?: boolean;
  /** Optional action label + handler (e.g. 撤销). */
  readonly actionLabel?: string;
  readonly onAction?: () => void;
  /**
   * A real expiry for an action toast. The prototype leaves an action toast open
   * until it is clicked, which is right for "已加入队列" but not for the
   * 10-second 撤销 window (task 10.8) — that window has to close on time.
   */
  readonly autoDismissMs?: number;
}

export interface ToastState extends ToastRequest {
  /** Monotonic id so a repeated identical message still restarts the timer. */
  readonly id: number;
}

type Listener = () => void;

class ToastStore {
  private current: ToastState | null = null;
  private seq = 0;
  private listeners = new Set<Listener>();

  get(): ToastState | null {
    return this.current;
  }

  show(request: ToastRequest): void {
    this.seq += 1;
    this.current = { ...request, id: this.seq };
    this.emit();
  }

  dismiss(): void {
    if (this.current === null) return;
    this.current = null;
    this.emit();
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private emit(): void {
    for (const listener of this.listeners) listener();
  }
}

export const toastStore = new ToastStore();

/** Render from the current toast. */
export function useToast(): ToastState | null {
  return useSyncExternalStore(
    (cb) => toastStore.subscribe(cb),
    () => toastStore.get(),
  );
}

/** Imperative helper for event handlers. */
export function notify(request: ToastRequest | string): void {
  toastStore.show(typeof request === "string" ? { message: request } : request);
}
