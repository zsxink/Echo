#!/usr/bin/env node
// Injection proofs for the normalize-os-file-open-paths boundary, the new
// scenario-command governance gate, and the audio-effects evidence discipline
// (its retraction of the withdrawn runtime-parameter conclusion).
//
// Kept out of `injection-suite.mjs` because that file sits right under the
// 1000-line scale ceiling; the suite spreads this cluster into ENTRIES.
//
// Every violation is injected by rewriting exactly one repository file
// (through the suite's `replaceIn`/`createProbe` helpers) and restored by hash.

/**
 * @param {{ replaceIn: Function, createProbe: Function, ROOT: string }} h
 *        the suite's reversible-mutation helpers and repository root
 */
export function governanceEntries({ replaceIn, createProbe, ROOT }) {
  return [
    {
      id: "file-open-normalization/raw-url-payload",
      guard: "the macOS file-open payload being handed downstream as a URL string instead of a decoded filesystem path",
      check: ["node", ["scripts/verify/checks/task-9.1.mjs"]],
      expect: /FAIL 9\.1: (echo-app clippy clean|main\.rs stringifies a URL again)/,
      baseline: true,
      inject() {
        // Regression to the original bug: the Opened payload is handed
        // downstream as a raw URL string (`.`->`to_string()`) instead of a
        // decoded filesystem path. The loop variable is deliberately spelled
        // `url` so the check's `url.to_string()` guard trips on every runner
        // (non-macOS exempts the dead-code lint on `open_targets`, so clippy
        // alone cannot prove the violation off-macOS).
        replaceIn(
          join(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"),
          "for path in open_targets::open_targets(&urls) {",
          "for path in urls.iter().map(|url| url.to_string()).map(std::path::PathBuf::from) {",
        );
        return {};
      },
    },
    {
      id: "scenario-command-proof/undeclared-hybrid",
      guard: "a scenario-backed hybrid check (delegates + repository-content assertions) with neither an injection proof nor a declaration",
      check: ["node", ["scripts/verify/check-scenario-command-proof.mjs"]],
      expect:
        /FAIL scenario-command-proof: 1 scenario-backed check\(s\) lack an executable failure proof[\s\S]*__injection_probe_hybrid/,
      baseline: true,
      inject() {
        // Point one scenario at a fresh hybrid check (spawns children AND
        // reads repository content) that has no entry and no declaration.
        replaceIn(
          join(ROOT, "scripts", "verify", "scenario-commands.mjs"),
          'CHECK("12.7")',
          'CHECK("__injection_probe_hybrid")',
        );
        createProbe(
          join(ROOT, "scripts", "verify", "checks", "task-__injection_probe_hybrid.mjs"),
          [
            "#!/usr/bin/env node",
            'import { spawnSync } from "node:child_process";',
            'import { readFileSync } from "node:fs";',
            'const out = spawnSync("cargo", ["check", "-p", "echo-app"], { encoding: "utf8" });',
            'if (out.status !== 0) { process.exit(1); }',
            'const src = readFileSync(new URL("../../apps/desktop/src-tauri/src/main.rs", import.meta.url), "utf8");',
            'if (!src.includes("StartupSupervisor")) { process.stderr.write("FAIL probe\\n"); process.exit(1); }',
            'process.stdout.write("ok probe\\n");',
            "",
          ].join("\n"),
        );
        return {};
      },
    },
    {
      id: "audio-effects-evidence/withdrawn-claim-reinstated",
      guard: "native-gate.md reinstating the withdrawn \"runtime parameter changes are categorically ineffective\" conclusion (its PCM experiment used the wrong mpv label)",
      check: ["node", ["scripts/verify/checks/task-audio-effects-evidence.mjs"]],
      expect: /native-gate\.md asserts runtime parameter changes are categorically ineffective/,
      baseline: true,
      inject() {
        // Delete the retraction marker, leaving the claim on its own line — the
        // exact edit that would resurrect the discredited conclusion. The row
        // still carries AFX-9.5 and RUNTIME_PARAM, so only the retraction
        // assertion can fire.
        replaceIn(
          join(ROOT, "openspec", "changes", "introduce-audio-effects-equalizer", "native-gate.md"),
          "**也已撤回**：其判别实验使用了错误的 mpv label。",
          "：其判别实验使用了错误的 mpv label。",
        );
        return {};
      },
    },
    {
      id: "audio-effects-dsp/normalization-enabled",
      guard: "the installed EQ enabling numerator normalization while the response model assumes it is disabled",
      check: ["node", ["scripts/verify/checks/audio-effects-dsp-math.mjs"]],
      expect: /no longer contains "normalize=false"/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "crates", "echo-desktop", "src", "player", "native_effects.rs"),
          "mix=1:normalize=false:precision=f64",
          "mix=1:normalize=true:precision=f64",
        );
        return {};
      },
    },
    {
      id: "audio-effects-dsp/undefined-width-returned-as-zero",
      guard: "the independent bandwidth solver silently returning a finite width when the relative threshold has no edges",
      check: ["node", ["scripts/verify/checks/audio-effects-dsp-selftest.mjs"]],
      expect: /below-threshold width is undefined/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "scripts", "verify", "checks", "audio-effects-dsp-reference.mjs"),
          "if (Math.abs(gainDb) <= distanceDb || centerHz > 0.45 * sampleRate) return null;",
          "if (Math.abs(gainDb) <= distanceDb || centerHz > 0.45 * sampleRate) return 0;",
        );
        return {};
      },
    },
    {
      id: "audio-effects-evidence/failed-command-labelled-confirmed",
      guard: "a negative runtime command result labelled as a confirmed PCM change",
      check: ["node", ["scripts/verify/checks/audio-effects-evidence-selftest.mjs"]],
      expect: /AFX-9\.5 lacks consistent pre-EOF PCM measurements for equalizer/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "openspec", "changes", "introduce-audio-effects-equalizer", "evidence", "macos-runtime-parameter-recheck-20261008.json"),
          '"command_result": 0',
          '"command_result": -12',
        );
        return {};
      },
    },
  ];
}

import { join } from "node:path";
