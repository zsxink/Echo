/**
 * The icon set shared by the desktop chrome.
 *
 * Every glyph is the one the prototype draws inline (`docs/prototype/
 * echo-desktop-player.html`), reproduced here so the React tree can compose
 * them — same paths, same 1.8px stroke for line icons and `fill` for transport
 * controls. Sizing and colouring come from the parent selector in the stylesheet
 * (`.nav-item svg`, `.control svg`, …), exactly as in the prototype, so no icon
 * carries its own dimensions.
 *
 * Conventions (brand §6):
 *  - 1.8px stroked line icons for navigation and menus.
 *  - Solid filled icons for transport (play/pause/previous/next) so control
 *    semantics stay distinguishable at a glance.
 *  - The favorite heart is the only glyph with an independent red semantic; it
 *    is stroked when unset and filled when set.
 */

import type { ReactNode } from "react";

interface Glyph {
  /** Nodes rendered inside the `<svg>` (paths/circles/text). */
  readonly nodes: ReactNode;
  /** Solid icons fill with `currentColor` and draw no stroke. */
  readonly solid?: boolean;
  /** The brand mark is drawn on its own 32×32 grid. */
  readonly box?: string;
  readonly strokeWidth?: number;
}

const GLYPHS: Record<string, Glyph> = {
  // --- brand ---------------------------------------------------------------
  note: {
    box: "0 0 32 32",
    solid: true,
    nodes: (
      <path d="M16.3 6.25v12.1c-1.62-.94-3.7-1.28-5.65-.76-3.9 1.02-6.02 4.58-4.76 7.42 1.2 2.75 5.27 3.3 8.82 1.2 2.65-1.57 4.35-4.08 4.79-6.36V9.1c3.45-.3 5.95-1.7 7.2-4.05V1.45C24.6 4.14 21.05 5.75 16.3 6.25Z" />
    ),
  },

  // --- sidebar navigation --------------------------------------------------
  library: { nodes: <path d="M4 6h16M4 12h16M4 18h16" /> },
  recent: {
    nodes: (
      <>
        <path d="M3 12a9 9 0 1 0 3-6.7M3 4v5h5" />
        <path d="M12 7v5l3 2" />
      </>
    ),
  },
  heart: {
    nodes: (
      <path d="M20.8 4.6a5.4 5.4 0 0 0-7.6 0L12 5.8l-1.2-1.2a5.4 5.4 0 1 0-7.6 7.6L12 21l8.8-8.8a5.4 5.4 0 0 0 0-7.6Z" />
    ),
  },
  plus: { nodes: <path d="M12 5v14M5 12h14" /> },

  // --- topbar / toolbar ----------------------------------------------------
  search: {
    strokeWidth: 2,
    nodes: (
      <>
        <circle cx="11" cy="11" r="6" />
        <path d="m20 20-4.2-4.2" />
      </>
    ),
  },
  sort: {
    strokeWidth: 1.7,
    nodes: (
      <>
        <text x="2.6" y="9" fill="currentColor" stroke="none" fontSize="7" fontWeight="700">
          A
        </text>
        <text x="2.6" y="20" fill="currentColor" stroke="none" fontSize="7" fontWeight="700">
          Z
        </text>
        <path d="M14 4v15m-3-3 3 3 3-3" />
      </>
    ),
  },
  check: { strokeWidth: 2.4, nodes: <path d="m5 12 4.2 4.2L19 6.8" /> },
  more: {
    solid: true,
    nodes: (
      <>
        <circle cx="5.5" cy="12" r="1.6" />
        <circle cx="12" cy="12" r="1.6" />
        <circle cx="18.5" cy="12" r="1.6" />
      </>
    ),
  },
  sidebar: { nodes: <path d="M6 5v14M12 5v14M18 5v14" /> },
  menu: { nodes: <path d="M4 6h16M4 12h16M4 18h16" /> },
  close: { nodes: <path d="m6 6 12 12M18 6 6 18" /> },
  chevronDown: { strokeWidth: 2, nodes: <path d="m6 9 6 6 6-6" /> },
  drag: { nodes: <path d="M4 9h16M4 15h16" /> },

  // --- transport -----------------------------------------------------------
  play: { solid: true, nodes: <path d="m8 5 11 7-11 7V5Z" /> },
  pause: { solid: true, nodes: <path d="M7 5h3v14H7zm7 0h3v14h-3z" /> },
  previous: { solid: true, nodes: <path d="M6 5h2v14H6zm3 7 9-7v14z" /> },
  next: { solid: true, nodes: <path d="M16 5h2v14h-2zM7 5l9 7-9 7z" /> },
  modeSequence: {
    nodes: (
      <>
        <path d="M17 3l3 3-3 3M20 6H9a5 5 0 0 0-5 5" />
        <path d="M7 21l-3-3 3-3M4 18h11a5 5 0 0 0 5-5" />
      </>
    ),
  },
  modeShuffle: {
    // 原型 playbackModes 的 shuffle 图标是唯一的实心曲线字形（sequence/single
    // 均为 1.8px 描边），照搬其 path —— 曲线交叉 + 实心箭头，不是描边直线。
    solid: true,
    nodes: (
      <path d="M3.5 6.5H6c2.22 0 3.42 1.72 4.77 3.48l1.05 1.37 1.16-1.42C14.3 8.3 15.5 6.5 17.72 6.5h1.48V4l3.3 3.5-3.3 3.5V8.5h-1.48c-1.24 0-2.1.9-3.28 2.42L13.22 12l1.22 1.58C15.62 15.1 16.48 16 17.72 16h1.48v-2.5l3.3 3.5-3.3 3.5V18h-1.48c-2.22 0-3.42-1.8-4.74-3.43l-1.16-1.44-1.05 1.38C9.42 16.28 8.22 18 6 18H3.5v-2H6c1.24 0 2.1-.9 3.28-2.42L10.5 12 9.28 10.42C8.1 8.9 7.24 8 6 8H3.5V6.5Z" />
    ),
  },
  modeRepeatOne: {
    nodes: (
      <>
        <path d="M17 3l3 3-3 3M20 6H9a5 5 0 0 0-5 5" />
        <path d="M7 21l-3-3 3-3M4 18h11a5 5 0 0 0 5-5" />
        <text x="9.4" y="15" fill="currentColor" stroke="none" fontSize="8" fontWeight="700">
          1
        </text>
      </>
    ),
  },
  volume: {
    nodes: (
      <>
        <path d="M4 10v4h4l5 4V6l-5 4H4z" />
        <path d="M17 9a4 4 0 0 1 0 6M19 6a8 8 0 0 1 0 12" />
      </>
    ),
  },
  volumeMuted: {
    nodes: (
      <>
        <path d="M4 10v4h4l5 4V6l-5 4H4z" />
        <path d="m16.5 9.5 5 5M21.5 9.5l-5 5" />
      </>
    ),
  },
  queue: {
    nodes: (
      <>
        <path d="M4 6h12M4 12h12M4 18h8" />
        <path d="m18 16 3 2-3 2v-4Z" />
      </>
    ),
  },

  // --- song menu / dialogs -------------------------------------------------
  playNext: {
    nodes: (
      <>
        <path d="M4 6h11M4 12h11M4 18h8" />
        <path d="m17 16 3 2-3 2v-4Z" />
      </>
    ),
  },
  info: {
    nodes: (
      <>
        <circle cx="12" cy="12" r="8" />
        <path d="M12 10v6M12 7h.01" />
      </>
    ),
  },
  folder: { nodes: <path d="M3 7h7l2 2h9v10H3z" /> },
  trash: {
    nodes: <path d="M5 7h14M10 11v5M14 11v5M8 7l1-3h6l1 3M7 7l1 13h8l1-13" />,
  },
  edit: { nodes: <path d="M4 16.5V20h3.5L18.7 8.8l-3.5-3.5L4 16.5ZM13.8 6.7l3.5 3.5" /> },
  camera: {
    nodes: (
      <>
        <path d="M4 8h3l2-2h6l2 2h3v10H4z" />
        <circle cx="12" cy="13" r="3" />
      </>
    ),
  },
};

export function Icon({
  name,
  filled = false,
  className,
}: {
  name: string;
  filled?: boolean;
  className?: string;
}) {
  const glyph = GLYPHS[name] ?? GLYPHS.library;
  const box = glyph.box ?? "0 0 24 24";
  const solid = glyph.solid || filled;
  return (
    <svg
      className={className}
      viewBox={box}
      aria-hidden="true"
      fill={solid ? "currentColor" : "none"}
      stroke={solid ? "none" : "currentColor"}
      strokeWidth={solid ? undefined : (glyph.strokeWidth ?? 1.8)}
      strokeLinecap={solid ? undefined : "round"}
      strokeLinejoin={solid ? undefined : "round"}
    >
      {glyph.nodes}
    </svg>
  );
}
