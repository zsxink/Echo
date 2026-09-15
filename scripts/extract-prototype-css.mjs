#!/usr/bin/env node
// One-off helper: lift the prototype's <style> block verbatim into the desktop
// app's stylesheet layer, split by region so each file stays reviewable.
//
// The prototype (`docs/prototype/echo-desktop-player.html`) is the visual
// source of truth; this script guarantees the app ships the *same declarations*
// (no hand transcription drift). Re-run it after editing the prototype.
//
//   node scripts/extract-prototype-css.mjs

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");
const SRC = resolve(ROOT, "docs", "prototype", "echo-desktop-player.html");
const OUT_DIR = resolve(ROOT, "apps", "desktop", "src", "styles");

const html = readFileSync(SRC, "utf8");
const open = html.indexOf("<style>");
const close = html.indexOf("</style>");
if (open < 0 || close < 0) {
  process.stderr.write("prototype <style> block not found\n");
  process.exit(1);
}
const css = html.slice(open + "<style>".length, close);
const lines = css.split("\n");

/** Slice the style block by 1-based inclusive line numbers (relative to css). */
function region(from, to) {
  return lines.slice(from - 1, to).join("\n").replace(/\s+$/, "") + "\n";
}

// Anchor lines are located by selector so the script survives prototype edits
// that only shift line numbers.
function lineOf(needle, from = 0) {
  for (let i = from; i < lines.length; i += 1) {
    if (lines[i].startsWith(needle)) return i + 1;
  }
  throw new Error(`anchor not found: ${needle}`);
}

const L_RESET = lineOf("* { box-sizing");
const L_APP = lineOf(".app {");
const L_LIBRARY = lineOf(".library-head {");
const L_PLAYER = lineOf(".playing-cover {");
const L_RESPONSIVE = lineOf("@media (max-width: 1100px)");

const HEADER = (title, extra = "") =>
  [
    `/* ${title}`,
    " *",
    " * Verbatim from docs/prototype/echo-desktop-player.html (the visual source of",
    " * truth). Do not hand-edit declarations here: change the prototype and re-run",
    " * `node scripts/extract-prototype-css.mjs` so the app cannot drift from it.",
    extra ? ` * ${extra}` : null,
    " *",
    " * Declaration order is preserved. Imports follow the intended cascade:",
    " * tokens → shell → library → player → responsive → app-extras.",
    " */",
    "",
  ]
    .filter((l) => l !== null)
    .join("\n");

const ALIASES = `
/* --- App alias: the desktop IPC \`Theme\` keys are coral / cobalt / turquoise
 * (crates/echo-desktop/src/platform/local_state.rs). They map 1:1 onto the
 * prototype's wine / cobalt / green blocks above, so the rendered surface is
 * identical to the prototype — only the attribute spelling differs, because the
 * persisted preference predates this alignment. Keep these two declarations in
 * sync with the wine / green blocks. --- */
:root[data-echo-theme="coral"] {
  --accent: oklch(0.6528 0.2219 19.38);
  --surface-warm: oklch(0.9460 0.0160 19.38);
}

:root[data-echo-theme="turquoise"] {
  --accent: oklch(0.54 0.1330 159.56);
  --accent-strong: oklch(0.44 0.1330 159.56);
  --accent-strong-hover: color-mix(in oklab, var(--accent-strong), black 8%);
  --surface-warm: oklch(0.9460 0.0180 159.56);
}
`;

const files = {
  "tokens.css": HEADER(
    "Echo design tokens, three-theme system and base reset.",
    "Theme blocks use the prototype ids: wine (默认珊瑚玫红) / cobalt / green.",
  ),
  "shell.css": HEADER(
    "Application frame: sidebar, workspace topbar, settings dialog, toast and",
    "confirmation dialog.",
  ),
  "library.css": HEADER(
    "Library workspace: view head, sort popover, track table, song menu and the",
    "playlist surfaces.",
  ),
  "player.css": HEADER(
    "Persistent player bar, immersive now-playing surface, queue popover and the",
    "lyrics column.",
  ),
  "responsive.css": HEADER("Responsive overrides (≤1100px / ≤760px)."),
};

// --- assemble regions -------------------------------------------------------
// tokens: comment + :root + theme blocks + base reset (up to just before `.app`)
// shell: application frame + topbar + settings/toast/confirmation surfaces
// library: library head, sort popover, track table, song menu, playlists
// player: record, lyrics, player bar, queue, now-playing surface
// responsive: every width-based media query at the tail of the prototype block
const tokens = region(1, L_APP - 1);
const shell = region(L_APP, L_LIBRARY - 1);
const library = region(L_LIBRARY, L_PLAYER - 1);
const player = region(L_PLAYER, L_RESPONSIVE - 1);
const responsive = region(L_RESPONSIVE, lines.length);

void L_RESET;

mkdirSync(OUT_DIR, { recursive: true });
const write = (name, body) => {
  writeFileSync(resolve(OUT_DIR, name), files[name] + body);
  return body.split("\n").length;
};

const counts = {
  "tokens.css": write("tokens.css", tokens + ALIASES),
  "shell.css": write("shell.css", shell),
  "library.css": write("library.css", library),
  "player.css": write("player.css", player),
  "responsive.css": write("responsive.css", responsive),
};

const total = Object.entries(counts)
  .map(([name, n]) => `${name} (${n} lines)`)
  .join(", ");
process.stdout.write(`extracted ${lines.length} prototype CSS lines → ${total}\n`);
