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

import { ExternalStore, useExternalStore } from "./externalStore";

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

class ToastStore {
  private readonly state = new ExternalStore<ToastState | null>(null);
  private seq = 0;

  get(): ToastState | null {
    return this.state.getSnapshot();
  }

  show(request: ToastRequest): void {
    this.seq += 1;
    this.state.setSnapshot({ ...request, id: this.seq });
  }

  dismiss(): void {
    if (this.state.getSnapshot() === null) return;
    this.state.setSnapshot(null);
  }

  subscribe(listener: () => void): () => void {
    return this.state.subscribe(listener);
  }

  stateForRender(): ExternalStore<ToastState | null> {
    return this.state;
  }
}

export const toastStore = new ToastStore();

/** Render from the current toast. */
export function useToast(): ToastState | null {
  return useExternalStore(toastStore.stateForRender());
}

/** Imperative helper for event handlers. */
export function notify(request: ToastRequest | string): void {
  toastStore.show(typeof request === "string" ? { message: request } : request);
}
