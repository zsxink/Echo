#!/usr/bin/env node
// Injection proofs for the normalize-os-file-open-paths boundary and the new
// scenario-command governance gate.
//
// Kept out of `injection-suite.mjs` because that file sits right under the
// 1000-line scale ceiling; the suite spreads this cluster into ENTRIES.
//
// Both violations are injected by rewriting exactly one repository file
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
        replaceIn(
          join(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"),
          "for path in open_targets::open_targets(&urls) {",
          "for path in urls.iter().map(|u| u.to_string()).map(std::path::PathBuf::from) {",
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
  ];
}

import { join } from "node:path";
