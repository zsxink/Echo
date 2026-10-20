/**
 * The anchor element of the effects trigger, shared with the panel so it can
 * restore focus when the popover closes. A module-level slot rather than a DOM
 * query keeps the two components decoupled.
 */
export const effectsTriggerRef = { current: null as HTMLElement | null };
