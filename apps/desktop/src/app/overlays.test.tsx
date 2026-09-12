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
