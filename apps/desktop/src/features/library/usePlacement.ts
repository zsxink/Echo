import { useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from "react";
import type { MenuAnchor } from "./SongMenu";

/** The prototype's own placement constants. */
const MENU_WIDTH = 248;
const MENU_HEIGHT = 254;
const VIEWPORT_GAP = 12;

/**
 * Anchored placement, using the prototype's own clamp (it never flips above the
 * trigger — it just stays inside the viewport). Measured height is used when
 * available so a long menu stays clickable.
 */
export function usePlacement(
  anchor: MenuAnchor | null | undefined,
  menuRef: RefObject<HTMLElement | null>,
): CSSProperties {
  const [height, setHeight] = useState(0);
  const measured = useRef(false);

  useLayoutEffect(() => {
    if (measured.current || !menuRef.current) return;
    const box = menuRef.current.getBoundingClientRect();
    if (box.height === 0) return;
    measured.current = true;
    setHeight(box.height);
  }, [menuRef]);

  if (!anchor) return {};
  const viewportW = window.innerWidth || 1280;
  const viewportH = window.innerHeight || 800;
  const boxHeight = height > 0 ? height : MENU_HEIGHT;
  return {
    left: Math.max(
      VIEWPORT_GAP,
      Math.min(viewportW - MENU_WIDTH - VIEWPORT_GAP, anchor.right - MENU_WIDTH),
    ),
    top: Math.max(VIEWPORT_GAP, Math.min(viewportH - boxHeight - VIEWPORT_GAP, anchor.bottom + 6)),
  };
}
