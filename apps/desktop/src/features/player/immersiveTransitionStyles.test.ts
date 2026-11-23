/**
 * Issue #85 — static assertions for the stylesheet half of the transition.
 *
 * The component tests prove the state machine; these read the shipped CSS and
 * pin the contract the visuals depend on: the exit token exists, both phase
 * selectors and the FLIP variables are present, the focus layout fades instead
 * of using `display:none`, and reduced motion is handled in CSS as well as in
 * JavaScript. Generated stylesheets are still checked byte-for-byte against the
 * prototype by `scripts/verify/checks/prototype-css-drift.mjs`; this file only
 * makes sure the app-owned layer did not drift from the design.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const read = (file: string) => readFileSync(resolve(process.cwd(), "src/styles", file), "utf8");

describe("immersive transition styles", () => {
  it("publishes the exit token in the generated tokens", () => {
    expect(read("tokens.css")).toMatch(/--motion-exit:\s*180ms/);
    expect(read("tokens.css")).toMatch(/--motion-base:\s*220ms/);
  });

  it("styles both painted phases and both FLIP variable families", () => {
    const player = read("player.css");
    expect(player).toContain('.now-playing-popover[data-phase="opening"]');
    expect(player).toContain('.now-playing-popover[data-phase="closing"]');
    expect(player).toContain(".now-playing-popover[data-measuring]");
    expect(player).toContain("--flip-x");
    expect(player).toContain("--flip-exit-x");
    expect(player).toMatch(/-motion-exit/);
  });

  it("only animates transform/opacity for the surface and cover", () => {
    const player = read("player.css");
    const surface = player.match(/\.now-playing-popover \{[^}]*transition:[^;]*;?\s*\}/)?.[0] ?? "";
    expect(surface).toContain("opacity");
    expect(surface).toContain("transform");
    expect(surface).not.toMatch(/filter|height|font-size/);
  });

  it("hides the focused metadata by visibility/opacity, never display:none", () => {
    const player = read("player.css");
    const focusRule =
      player.match(
        /body\.player-mode-open\.lyrics-focus-open \.now-playing-popover \.playing-cover,[\s\S]*?\{([\s\S]*?)\}/,
      )?.[1] ?? "";
    expect(focusRule).not.toBe("");
    expect(focusRule).not.toContain("display: none");
    expect(focusRule).toContain("visibility: hidden");
    expect(focusRule).toContain("opacity: 0");
    expect(focusRule).toContain("transition");
  });

  it("drives the colour variables at the entry/exit rhythm and clamps reduce", () => {
    const artwork = read("artwork.css");
    expect(artwork).toContain("body.player-mode-open.player-mode-entering");
    expect(artwork).toContain("body.player-mode-open.player-mode-closing");
    expect(artwork).toMatch(/--player-tint var\(--motion-base\)/);
    expect(artwork).toMatch(/--player-tint var\(--motion-exit\)/);
    const reduce =
      artwork.match(/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*?\}\s*\}/)?.[0] ?? "";
    expect(reduce).toContain("body.player-mode-open.player-mode-entering");
    expect(reduce).toContain("body.player-mode-open.player-mode-closing");
  });

  it("crossfades the record label without animating layout", () => {
    const extras = read("app-extras.css");
    expect(extras).toContain(".ring-art.is-crossfading-in");
    expect(extras).toContain("@keyframes ring-art-in");
    const keyframes = extras.match(/@keyframes ring-art-in \{([\s\S]*?)\}/)?.[1] ?? "";
    expect(keyframes).toContain("opacity");
    expect(keyframes).not.toMatch(/height|width|font-size/);
    expect(extras).toMatch(
      /@media \(prefers-reduced-motion: reduce\) \{[\s\S]*?\.ring-art\.is-crossfading-in[\s\S]*?animation: none/,
    );
  });

  it("keeps the record rotation exempt under reduced motion", () => {
    expect(read("responsive.css")).toMatch(
      /prefers-reduced-motion: reduce[\s\S]*?\.playing-cover\.is-playing \.ring \{ animation: record-spin 12s linear infinite !important; \}/,
    );
  });
});
