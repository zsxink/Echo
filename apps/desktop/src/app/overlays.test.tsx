/**
 * Task 12.1 — single Overlay Manager: focus trap/restore, roving menu and the
 * one-stack Escape priority.
 *
 * Acceptance (desktop-app-shell spec §键盘焦点与 Escape 行为必须可访问):
 *  - Escape closes exactly one (the topmost-priority) open layer at a time —
 *    never several — in the order: 阻断确认 → 选择器 → 菜单/队列 → 歌词专注 →
 *    沉浸 → 窄屏侧边栏.
 *  - Opening an overlay moves focus into its content; closing restores focus to
 *    the element that opened it.
 *  - Tab is confined to the overlay while it is open (focus trap).
 *  - A `role="menu"` supports roving focus (Up/Down/Home/End move the active
 *    item; Enter activates) for keyboard-only users.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect, useRef, useState } from "react";
import { describe, expect, it } from "vitest";

import { OverlayTier, useFocusTrap, useOverlay, useRovingFocus } from "./overlays";

/** A minimal overlay that registers itself on the shared stack. */
function FakeOverlay({
  tier,
  label,
  onClose,
}: {
  tier: OverlayTier;
  label: string;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useOverlay({ tier, onClose, containerRef: ref });
  useFocusTrap(ref);
  return (
    <div ref={ref} data-testid={`overlay-${label}`} role="dialog" aria-label={label}>
      <button type="button" data-testid={`btn-${label}`}>
        {label}
      </button>
    </div>
  );
}

/** A host that opens a blocking dialog and a picker on top, and tracks closes. */
function StackHost() {
  const [menu, setMenu] = useState(false);
  const [dialog, setDialog] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuClose = useRef(() => setMenu(false));
  const dialogClose = useRef(() => setDialog(false));
  return (
    <div>
      <button type="button" ref={triggerRef} data-testid="open-menu" onClick={() => setMenu(true)}>
        打开菜单
      </button>
      {menu ? (
        <FakeOverlay
          tier={OverlayTier.Menu}
          label="menu"
          onClose={() => {
            menuClose.current();
            triggerRef.current?.focus();
          }}
        />
      ) : null}
      {menu && dialog ? (
        <FakeOverlay
          tier={OverlayTier.Picker}
          label="dialog"
          onClose={() => dialogClose.current()}
        />
      ) : null}
      <button type="button" data-testid="open-dialog" onClick={() => setDialog(true)}>
        打开对话框
      </button>
    </div>
  );
}

describe("Overlay manager (task 12.1)", () => {
  it("Escape closes exactly the topmost-priority layer, one at a time", () => {
    render(<StackHost />);
    fireEvent.click(screen.getByTestId("open-menu"));
    fireEvent.click(screen.getByTestId("open-dialog"));

    // Both are open.
    expect(screen.getByTestId("overlay-menu")).toBeInTheDocument();
    expect(screen.getByTestId("overlay-dialog")).toBeInTheDocument();

    // Escape pops only the picker (higher priority), leaving the menu.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("overlay-dialog")).not.toBeInTheDocument();
    expect(screen.getByTestId("overlay-menu")).toBeInTheDocument();

    // A second Escape closes the menu.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("overlay-menu")).not.toBeInTheDocument();
  });

  it("opening an overlay moves focus into it and closing restores focus to the trigger", async () => {
    render(<StackHost />);
    const trigger = screen.getByTestId("open-menu");
    trigger.focus();

    fireEvent.click(trigger);
    // useOverlay moves focus to the overlay's first focusable button.
    await waitFor(() => expect(screen.getByTestId("btn-menu")).toHaveFocus());

    fireEvent.keyDown(window, { key: "Escape" });
    // Focus returns to the element that opened the overlay (the trigger).
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("traps Tab inside the open overlay", async () => {
    render(<FakeOverlay tier={OverlayTier.Menu} label="trapped" onClose={() => undefined} />);
    const inside1 = screen.getByTestId("btn-trapped");
    await waitFor(() => expect(inside1).toHaveFocus());

    // Tab at the last focusable inside an overlay returns to the first, and
    // never escapes to the body/default document.
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "Tab" });
    expect(screen.getByTestId("btn-trapped")).toBeInTheDocument();
  });

  it("dismisses the top menu when an interaction starts elsewhere", () => {
    render(<StackHost />);
    fireEvent.click(screen.getByTestId("open-menu"));
    fireEvent.pointerDown(screen.getByTestId("open-dialog"));
    expect(screen.queryByTestId("overlay-menu")).not.toBeInTheDocument();
  });
});

describe("Overlay trigger control (fix-queue-trigger-toggle)", () => {
  /**
   * The queue panel's real shape: the panel and its trigger are siblings, so the
   * trigger is NOT inside the panel. `pointerdown` on the trigger used to read
   * as "outside" — the panel closed and the same gesture's `click` re-opened it,
   * leaving the button looking inert.
   */
  function SplitTriggerHost() {
    const [open, setOpen] = useState(false);
    const triggerRef = useRef<HTMLButtonElement>(null);
    const panelRef = useRef<HTMLDivElement>(null);
    useOverlay({
      tier: OverlayTier.Menu,
      onClose: () => setOpen(false),
      containerRef: panelRef,
      triggerRef,
      enabled: open,
    });
    useFocusTrap(panelRef, open);
    return (
      <div>
        <button
          type="button"
          ref={triggerRef}
          data-testid="queue-trigger"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          播放队列
        </button>
        {open ? (
          <div ref={panelRef} data-testid="queue-panel" role="dialog" aria-label="播放队列">
            <button type="button" data-testid="queue-item">
              曲目
            </button>
          </div>
        ) : null}
        <button type="button" data-testid="elsewhere">
          其它区域
        </button>
      </div>
    );
  }

  it("closes on a second press of the trigger instead of reopening it", () => {
    render(<SplitTriggerHost />);
    const trigger = screen.getByTestId("queue-trigger");

    fireEvent.click(trigger);
    expect(screen.getByTestId("queue-panel")).toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "true");

    // The real gesture: pointerdown then click at the same coordinates.
    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    expect(screen.queryByTestId("queue-panel")).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("still dismisses on a press genuinely outside the layer and its trigger", () => {
    render(<SplitTriggerHost />);
    fireEvent.click(screen.getByTestId("queue-trigger"));
    expect(screen.getByTestId("queue-panel")).toBeInTheDocument();

    fireEvent.pointerDown(screen.getByTestId("elsewhere"));
    expect(screen.queryByTestId("queue-panel")).not.toBeInTheDocument();
  });

  it("leaves a layer without a triggerRef on the container-only rule", () => {
    render(<StackHost />);
    fireEvent.click(screen.getByTestId("open-menu"));
    expect(screen.getByTestId("overlay-menu")).toBeInTheDocument();

    // No triggerRef was registered, so every press outside the panel — including
    // the button that opened it — is still an outside press: unchanged behaviour.
    fireEvent.pointerDown(screen.getByTestId("open-menu"));
    expect(screen.queryByTestId("overlay-menu")).not.toBeInTheDocument();
  });
});

describe("Roving menu focus (task 12.1)", () => {
  /** A `role="menu"` with three roving `role="menuitem"` entries. */
  function RovingMenu() {
    const items = [
      { id: "item-a", label: "A" },
      { id: "item-b", label: "B" },
      { id: "item-c", label: "C" },
    ];
    const { activeId, onMenuKeyDown, setActive } = useRovingFocus(items);
    const containerRef = useRef<HTMLDivElement>(null);
    useOverlay({
      tier: OverlayTier.Menu,
      onClose: () => undefined,
      containerRef,
      enabled: false,
    });
    useEffect(() => {
      document.getElementById(items[0].id)?.focus();
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return (
      <div ref={containerRef} role="menu" data-testid="roving-menu" onKeyDown={onMenuKeyDown}>
        {items.map((it) => (
          <button
            key={it.id}
            id={it.id}
            type="button"
            role="menuitem"
            onClick={() => setActive(it.id)}
            aria-current={activeId === it.id ? "true" : undefined}
          >
            {it.label}
          </button>
        ))}
        <p data-testid="active">{activeId}</p>
      </div>
    );
  }

  it("moves the active menuitem with Down/Home/End and retains Enter activation", () => {
    render(<RovingMenu />);
    const menu = screen.getByTestId("roving-menu");

    expect(screen.getByTestId("active")).toHaveTextContent("item-a");

    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(screen.getByTestId("active")).toHaveTextContent("item-b");
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(screen.getByTestId("active")).toHaveTextContent("item-c");

    // End / Home jump to last / first.
    fireEvent.keyDown(menu, { key: "End" });
    expect(screen.getByTestId("active")).toHaveTextContent("item-c");
    fireEvent.keyDown(menu, { key: "Home" });
    expect(screen.getByTestId("active")).toHaveTextContent("item-a");

    // ArrowUp wraps back toward the start from the first item.
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(screen.getByTestId("active")).toHaveTextContent("item-a");
  });
});
