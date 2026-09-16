import { useCallback, useLayoutEffect, useRef, useState } from "react";

import { Icon } from "../../app/Icon";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { SORT_FIELDS, type SongSort } from "./types";

interface SongSortControlProps {
  readonly sort: SongSort;
  readonly onChange: (sort: SongSort) => void;
}

/** The shared compact sort menu used by song views. */
export function SongSortControl({ sort, onChange }: SongSortControlProps) {
  const [open, setOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ left: 0, top: 0, maxHeight: 0 });
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const positionMenu = useCallback(() => {
    const anchor = buttonRef.current?.getBoundingClientRect();
    if (!anchor) return;
    const margin = 12;
    const gap = 8;
    const desiredHeight = 284;
    const below = window.innerHeight - anchor.bottom - gap - margin;
    const above = anchor.top - gap - margin;
    const openBelow = below >= desiredHeight || below >= above;
    const available = Math.max(120, openBelow ? below : above);
    setMenuPosition({
      left: Math.max(margin, anchor.right - 160),
      top: openBelow
        ? anchor.bottom + gap
        : Math.max(margin, anchor.top - gap - Math.min(desiredHeight, available)),
      maxHeight: available,
    });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    positionMenu();
    window.addEventListener("resize", positionMenu);
    window.addEventListener("scroll", positionMenu, true);
    return () => {
      window.removeEventListener("resize", positionMenu);
      window.removeEventListener("scroll", positionMenu, true);
    };
  }, [open, positionMenu]);
  useOverlay({
    tier: OverlayTier.Menu,
    onClose: () => setOpen(false),
    containerRef: wrapRef,
    enabled: open,
  });
  useFocusTrap(wrapRef, open);

  return (
    <div className="sort-wrap" ref={wrapRef}>
      <button
        type="button"
        className="tool-button tool-icon"
        ref={buttonRef}
        aria-label="排序方式"
        title="排序方式"
        aria-expanded={open}
        onClick={() => {
          if (!open) positionMenu();
          setOpen((value) => !value);
        }}
        data-testid="sort-button"
      >
        <Icon name="sort" />
      </button>
      <div
        className="sort-popover"
        role="menu"
        aria-label="排序选项"
        hidden={!open}
        style={{
          position: "fixed",
          left: menuPosition.left,
          right: "auto",
          top: menuPosition.top,
          width: 160,
          maxHeight: menuPosition.maxHeight,
          overflowY: "auto",
        }}
      >
        {SORT_FIELDS.map((field) => (
          <button
            key={field.value}
            type="button"
            role="menuitemradio"
            aria-checked={field.value === sort.field}
            className={`sort-option${field.value === sort.field ? " active" : ""}`}
            onClick={() => {
              onChange({ ...sort, field: field.value });
              setOpen(false);
            }}
          >
            <Icon className="sort-check" name="check" />
            <span>{field.label}</span>
          </button>
        ))}
        <div className="sort-divider" role="separator" />
        {(["asc", "desc"] as const).map((direction) => (
          <button
            key={direction}
            type="button"
            role="menuitemradio"
            aria-checked={direction === sort.direction}
            className={`sort-option${direction === sort.direction ? " active" : ""}`}
            data-sort-direction={direction}
            onClick={() => {
              onChange({ ...sort, direction });
              setOpen(false);
            }}
          >
            <Icon className="sort-check" name="check" />
            <span>{direction === "asc" ? "升序" : "降序"}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
