import { useSyncExternalStore } from "react";

/**
 * Shared external-state primitive.
 *
 * React-facing state in the desktop app is kept outside component trees only
 * when multiple surfaces genuinely share it. Every such store exposes the same
 * immutable snapshot plus `subscribe` contract, so `useSyncExternalStore`
 * remains the sole rendering boundary.
 */

export type StoreListener = () => void;

export class ExternalStore<T> {
  private snapshot: T;
  private readonly listeners = new Set<StoreListener>();

  constructor(initialSnapshot: T) {
    this.snapshot = initialSnapshot;
  }

  getSnapshot(): T {
    return this.snapshot;
  }

  setSnapshot(next: T): void {
    if (Object.is(this.snapshot, next)) return;
    this.snapshot = next;
    this.emit();
  }

  update(updater: (current: T) => T): void {
    this.setSnapshot(updater(this.snapshot));
  }

  subscribe(listener: StoreListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(): void {
    for (const listener of this.listeners) listener();
  }
}

/** Render a shared store through React's one external-store boundary. */
export function useExternalStore<T, Selected = T>(
  store: ExternalStore<T>,
  select: (snapshot: T) => Selected = (snapshot) => snapshot as unknown as Selected,
): Selected {
  return useSyncExternalStore(
    (listener) => store.subscribe(listener),
    () => select(store.getSnapshot()),
  );
}
