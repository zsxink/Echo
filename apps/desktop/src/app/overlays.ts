/**
 * Single overlay manager (task 12.1).
 *
 * Echo renders several kinds of floating layers: blocking confirm dialogs,
 * pickers (add-to-playlist, settings), menus (sort, song actions), the queue
 * panel, the lyrics-focus surface, the immersive player, and the narrow-screen
 * sidebar. Before this module each layer owned its own Escape handler and no
 * layer managed focus, so Escape could close several layers at once and Tab
 * could escape a dialog into the page behind it.
 *
 * This module provides **one** overlay stack with a **fixed priority order**
 * (spec: "Escape SHALL 由单一浮层栈按…优先级关闭一个界面"). Escape closes exactly
 * the highest-priority layer that is currently open — never several at once.
 * Each overlay registers itself through {@link useOverlay}, which also:
 *
 *  - **Reveals trap focus**: while open, Tab/Shift+Tab are confined to the
 *    overlay and focus moves to its content on open ({@link useFocusTrap}).
 *  - **Restores focus**: closing returns focus to the element that opened the
 *    overlay (the trigger), so the interaction continues where it left off.
 *
 * {@link useRovingFocus} implements the WAI-ARIA "roving tabindex" pattern for
 * `role="menu"` / `role="menuitem"` groups (Up/Down move the active item,
 * Home/End jump, Enter/Space activate) — used by the song action menu.
 *
 * The tier order is the authoritative spec list (highest priority first):
 * blocking confirm / named dialogs → settings / playlist pickers → menus /
 * sort / queue → lyrics focus → immersive player → narrow sidebar.
 */

import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";

/** Stack layers in the spec's Escape-priority order (highest priority first). */
export enum OverlayTier {
  /** 阻断确认/命名对话框：delete confirm, import dialog. */
  BlockingDialog = 10,
  /** 设置或歌单选择器：add-to-playlist picker, read-only song detail. */
  Picker = 20,
  /** 菜单/排序/播放队列：song action menu, queue panel (same tier). */
  Menu = 30,
  /** 歌词专注 surface. */
  LyricsFocus = 40,
  /** 沉浸式播放器. */
  Immersive = 50,
  /** 窄屏侧边栏. */
  Sidebar = 60,
}

interface Registration {
  readonly tier: OverlayTier;
  readonly closer: () => void;
  readonly containerRef: RefObject<HTMLElement | null>;
  /** The control that opened this layer, when it sits outside the container. */
  readonly triggerRef?: RefObject<HTMLElement | null>;
  readonly dismissOnInteractOutside: boolean;
  readonly preserveOutsideFocus: boolean;
  preserveFocus: boolean;
  readonly seq: number;
}

/**
 * Whether a pointer target counts as *inside* a layer: its own container, or
 * the control that opened it.
 *
 * The trigger is a separate ref rather than part of `containerRef` because
 * triggers legitimately live outside the layer's own DOM — the queue button in
 * the player bar, a row's 歌曲操作 button in a virtualised table. Without it,
 * `pointerdown` on the trigger reads as "outside", dismisses the layer, and the
 * same gesture's `click` then re-opens it, so the control looks inert.
 *
 * Exported for the few components that run their own outside-press check (the
 * song menu suspends it while a nested dialog is up, which the stack cannot
 * know) so they stay on one definition of "inside".
 */
export function isInsideOverlay(
  containerRef: RefObject<HTMLElement | null>,
  triggerRef: RefObject<HTMLElement | null> | undefined,
  target: Node,
): boolean {
  return (
    containerRef.current?.contains(target) === true ||
    triggerRef?.current?.contains(target) === true
  );
}

const open = new Map<string, Registration>();
let nextId = 0;
let seqCounter = 0;

/** The highest-priority open tier, or null when the stack is empty. */
function topTier(): OverlayTier | null {
  if (open.size === 0) return null;
  let min = Infinity;
  for (const { tier } of open.values()) if (tier < min) min = tier;
  return min;
}

/** The id of the layer Escape should close: the newest layer at the top tier. */
function topId(): string | null {
  const target = topTier();
  if (target === null) return null;
  let latest: string | null = null;
  let latestSeq = -Infinity;
  for (const [id, reg] of open) {
    if (reg.tier !== target) continue;
    if (reg.seq > latestSeq) {
      latestSeq = reg.seq;
      latest = id;
    }
  }
  return latest;
}

let escapeInstalled = false;
/** One window-level Escape handler that pops the single topmost layer. */
function installEscapeHandler(): void {
  if (escapeInstalled) return;
  escapeInstalled = true;
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const id = topId();
    if (id !== null) {
      event.stopPropagation();
      const closer = open.get(id)?.closer;
      open.delete(id);
      closer?.();
    }
  });
}

// Installed once at module load — every overlay closes through this one
// handler, so Escape can never close several layers at once.
installEscapeHandler();

/** Menus are transient: one press in another surface dismisses the top menu
 * before that surface receives its own click. Dialogs keep their explicit
 * confirm/cancel contract and therefore opt out by default. */
window.addEventListener(
  "pointerdown",
  (event) => {
    const id = topId();
    const registration = id === null ? undefined : open.get(id);
    if (!registration?.dismissOnInteractOutside) return;
    const target = event.target as Node | null;
    if (target && isInsideOverlay(registration.containerRef, registration.triggerRef, target)) {
      return;
    }
    registration.preserveFocus = registration.preserveOutsideFocus;
    open.delete(id!);
    registration.closer();
  },
  true,
);

export interface OverlayOptions {
  readonly tier: OverlayTier;
  readonly onClose: () => void;
  /** A ref whose focusable content is trapped and initially focused. */
  readonly containerRef: RefObject<HTMLElement | null>;
  /**
   * The control that opened this overlay, when it is not a descendant of
   * `containerRef`. It counts as part of the layer's interior, so pressing it
   * again dismisses the layer instead of dismissing and immediately re-opening
   * it. Only affects the outside-press check — never Escape priority, focus
   * restore, or the Tab trap.
   */
  readonly triggerRef?: RefObject<HTMLElement | null>;
  /** When false the overlay is not registered on the stack (default true). */
  readonly enabled?: boolean;
  /** Close when the user begins an interaction outside this layer. Menus do so
   * by default; dialogs and full-screen surfaces require an explicit choice. */
  readonly dismissOnInteractOutside?: boolean;
  /** Nonmodal surfaces retain the target of outside clicks and focus changes. */
  readonly nonModal?: boolean;
}

/**
 * Registers an overlay on the single stack for the lifetime of the mounted
 * component: traps/reveals focus while open and restores it to the trigger on
 * close. Escape is handled once globally by the stack in priority order.
 */
export function useOverlay({
  tier,
  onClose,
  containerRef,
  triggerRef,
  enabled = true,
  dismissOnInteractOutside = tier === OverlayTier.Menu,
  nonModal = false,
}: OverlayOptions): void {
  const id = useRef(`o-${nextId++}`).current;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  // Remember the document.activeElement at open to restore it on close.
  const restoreFocus = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!enabled) return;
    restoreFocus.current =
      triggerRef?.current ?? (document.activeElement as HTMLElement | null) ?? null;
    seqCounter += 1;
    const registration: Registration = {
      tier,
      closer: () => onCloseRef.current(),
      containerRef,
      triggerRef,
      dismissOnInteractOutside,
      preserveOutsideFocus: nonModal,
      preserveFocus: false,
      seq: seqCounter,
    };
    open.set(id, registration);

    // Move focus into the overlay (spec: 打开时焦点进入其内容). Prefer an
    // autofocus element, else the first focusable, else the container.
    const el = containerRef.current;
    if (el) {
      const autofocus = el.querySelector<HTMLElement>("[autofocus]");
      const focusable = focusableIn(el);
      (autofocus ?? focusable?.[0] ?? el).focus();
    }

    const onFocusOutside = (event: FocusEvent) => {
      if (!nonModal || topId() !== id) return;
      const target = event.target as Node | null;
      if (!target || isInsideOverlay(containerRef, triggerRef, target)) return;
      registration.preserveFocus = true;
      open.delete(id);
      onCloseRef.current();
    };
    if (nonModal) document.addEventListener("focusin", onFocusOutside);

    return () => {
      document.removeEventListener("focusin", onFocusOutside);
      open.delete(id);
      // Restore focus to the trigger (spec: 关闭后焦点恢复到触发控件), only if it
      // is still in the document (not an element we unmounted this very call).
      const trigger = restoreFocus.current;
      if (
        !registration.preserveFocus &&
        trigger &&
        trigger.isConnected &&
        document.contains(trigger)
      ) {
        trigger.focus();
      }
    };
  }, [id, tier, enabled, containerRef, triggerRef, dismissOnInteractOutside, nonModal]);
}

/** Trap Tab/Shift+Tab inside a container so focus cannot escape an overlay. */
export function useFocusTrap(containerRef: RefObject<HTMLElement | null>, enabled = true): void {
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const el = containerRef.current;
      if (!el) return;
      const top = topId();
      if (top !== null && open.get(top)?.containerRef.current !== el) return;
      const items = focusableIn(el);
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement as Node | null;
      const inside = active !== null && el.contains(active);
      if (event.shiftKey) {
        if (active === first || !inside) {
          event.preventDefault();
          last.focus();
        }
      } else if (active === last || !inside) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown, true);
    return () => document.removeEventListener("keydown", onKeyDown, true);
  }, [containerRef, enabled]);
}

/** Select the focusable elements inside a container, in DOM order. */
function focusableIn(container: HTMLElement): HTMLElement[] {
  const selector = [
    "a[href]",
    "button:not([disabled])",
    'button[aria-disabled="false"]',
    "input:not([disabled])",
    "select:not([disabled])",
    "textarea:not([disabled])",
    "[tabindex]:not([tabindex='-1'])",
  ].join(",");
  return Array.from(container.querySelectorAll<HTMLElement>(selector)).filter((el) => {
    if (el.getAttribute("aria-hidden") === "true") return false;
    if (el.closest('[aria-hidden="true"]')) return false;
    return true;
  });
}

/**
 * WAI-ARIA "roving tabindex" for a `role="menu"` of `role="menuitem"` buttons.
 * Up/Down move the active item, Home/End jump to first/last, Enter/Space
 * activate the currently roved item. Returns the active item id and the key
 * handler to bind on the menu container.
 */
export function useRovingFocus(items: readonly { readonly id: string }[]): {
  activeId: string | null;
  onMenuKeyDown: (event: ReactKeyboardEvent) => void;
  setActive: (id: string) => void;
} {
  const [activeId, setActive] = useState<string | null>(items[0]?.id ?? null);

  function focusItem(id: string | null): void {
    setActive(id);
    if (id) document.getElementById(id)?.focus();
  }

  function onMenuKeyDown(event: ReactKeyboardEvent): void {
    const idx = items.findIndex((it) => it.id === activeId);
    switch (event.key) {
      case "ArrowDown": {
        event.preventDefault();
        focusItem(items[Math.min(items.length - 1, idx + 1)]?.id ?? null);
        break;
      }
      case "ArrowUp": {
        event.preventDefault();
        focusItem(items[Math.max(0, idx - 1)]?.id ?? null);
        break;
      }
      case "Home": {
        event.preventDefault();
        focusItem(items[0]?.id ?? null);
        break;
      }
      case "End": {
        event.preventDefault();
        focusItem(items[items.length - 1]?.id ?? null);
        break;
      }
      default:
        return;
    }
  }

  return { activeId, onMenuKeyDown, setActive };
}
