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
      id: "ci-platform-matrix/missing-linux-entry",
      guard: "the CI Rust platform matrix losing its Linux platform-to-runner mapping",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml rust matrix is missing platform 'Linux'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "          - platform: Linux\n            os: ubuntu-latest\n",
          "",
        );
        return {};
      },
    },
    {
      id: "ci-structure/test-condition-windows",
      guard: "the Rust Test step being gated to Windows instead of Linux on pull requests",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: rust Test if must include 'matrix\.platform == 'Linux''/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "if: github.event_name != 'pull_request' || matrix.platform == 'Linux'",
          "if: github.event_name != 'pull_request' || matrix.platform == 'Windows'",
        );
        return {};
      },
    },
    {
      id: "ci-structure/rust-static-step-missing",
      guard: "the Rust Linux job losing its rust-static/purity governance step",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'rust' is missing step 'Rust static gates'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Rust static gates\n        if: matrix.platform == 'Linux'\n        env:\n          ECHO_PURITY_REQUIRE_BUILD: \"1\"\n        run: pnpm verify:governance -- --sections=rust-static,purity\n\n",
          "",
        );
        return {};
      },
    },
    {
      id: "ci-structure/test-condition-and-inverted",
      guard: "the Rust Test tiering condition using && so PRs and main both lose the full workspace test",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: rust Test if must be 'github\.event_name != 'pull_request' \|\| matrix\.platform == 'Linux'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "if: github.event_name != 'pull_request' || matrix.platform == 'Linux'",
          "if: github.event_name != 'pull_request' && matrix.platform == 'Linux'",
        );
        return {};
      },
    },
    {
      id: "ci-structure/test-continue-on-error",
      guard: "the Rust Test step becoming advisory via continue-on-error: true",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: rust Test must not set continue-on-error/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Test\n        if: github.event_name != 'pull_request' || matrix.platform == 'Linux'\n",
          "      - name: Test\n        if: github.event_name != 'pull_request' || matrix.platform == 'Linux'\n        continue-on-error: true\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/governance-static-run-disabled",
      guard: "governance-static keeping the section flags but replacing the run with an echo",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static run must be 'pnpm verify:governance -- --sections=registry,static,coverage'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "        run: pnpm verify:governance -- --sections=registry,static,coverage",
          '        run: echo "--sections=registry,static,coverage"',
        );
        return {};
      },
    },
    {
      id: "ci-structure/perf-job-gated-off-pr",
      guard: "the perf job gaining a job-level if so performance scenarios stop running on pull requests",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'perf' must not be conditionally skipped/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "  perf:\n    name: Performance scenarios\n",
          "  perf:\n    name: Performance scenarios\n    if: github.event_name != 'pull_request'\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/section-coverage-missing",
      guard: "governance-static dropping the coverage section from its --sections selection",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static run must be 'pnpm verify:governance -- --sections=registry,static,coverage'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "--sections=registry,static,coverage",
          "--sections=registry,static",
        );
        return {};
      },
    },
    {
      id: "ci-structure/section-list-authority",
      guard: "a section disappearing from the selector and the workflow together, which a derived-only guard would not notice",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci-governance no longer lists section 'coverage'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "scripts", "verify", "ci-governance.mjs"),
          '"static", "coverage", "purity"',
          '"static", "purity"',
        );
        return {};
      },
    },
    {
      id: "release-platform-matrix/corrupt-linux-runner",
      guard: "the release Build matrix mapping Linux to a non-Linux runner",
      check: ["node", ["scripts/verify/checks/task-crp.mjs"]],
      expect: /FAIL CRP: release\.yml build platform 'linux' maps os to 'windows-latest', expected 'ubuntu-latest'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "release.yml"),
          "          - os: ubuntu-latest\n            arch: x64\n            platform: linux\n",
          "          - os: windows-latest\n            arch: x64\n            platform: linux\n",
        );
        return {};
      },
    },
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
          join(ROOT, "apps", "desktop", "src-tauri", "src", "bootstrap.rs"),
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
          'node("scripts/verify/checks/task-12.7.mjs")',
          'node("scripts/verify/checks/task-__injection_probe_hybrid.mjs")',
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
          join(ROOT, "openspec", "changes", "archive", "2026-10-13-introduce-audio-effects-equalizer", "native-gate.md"),
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
          join(ROOT, "openspec", "changes", "archive", "2026-10-13-introduce-audio-effects-equalizer", "evidence", "macos-runtime-parameter-recheck-20261008.json"),
          '"command_result": 0',
          '"command_result": -12',
        );
        return {};
      },
    },
    {
      id: "windows-installer/webview2-fallback-regressed-to-download",
      guard: "the Windows installer config losing its embedded WebView2 bootstrapper and regressing to the plaintext-HTTP downloadBootstrapper default (issue #64)",
      check: ["node", ["scripts/verify/checks/task-17.1.mjs"]],
      expect: /FAIL windows-installer: bundle\.windows\.webviewInstallMode\.type must be "embedBootstrapper"/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "apps", "desktop", "src-tauri", "tauri.windows.conf.json"),
          '      "webviewInstallMode": { "type": "embedBootstrapper", "silent": true },\n',
          "",
        );
        return {};
      },
    },
    {
      id: "licensing/spdx-regressed-to-busl",
      guard: "the workspace license regressing from the custom LicenseRef to the standard BUSL-1.1 SPDX id (issue #66: BUSL-1.1's base grant includes redistribution, which Echo Personal Study License deliberately does not)",
      check: ["node", ["scripts/verify/checks/task-18.1.mjs"]],
      expect: /FAIL 18\.1: [\s\S]*Cargo\.toml workspace license must be "LicenseRef-Echo-Personal-Study-1\.0"/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "Cargo.toml"),
          'license = "LicenseRef-Echo-Personal-Study-1.0"',
          'license = "BUSL-1.1"',
        );
        return {};
      },
    },
    {
      id: "scenario-evidence/missing-target",
      guard: "a scenario whose evidence names a test that does not exist",
      check: ["node", ["scripts/verify/check-scenario-evidence.mjs"]],
      expect: /FAIL scenario evidence[\s\S]*no_such_test/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "scripts", "verify", "scenario-commands.mjs"),
          'rust("infrastructure::sqlite::tests::catalog_all_songs_view")',
          'rust("no_such_test")',
        );
        return {};
      },
    },
    {
      id: "scenario-evidence/unobserved-target",
      guard: "a scenario whose evidence exists but its aggregated command never runs it",
      check: ["node", ["scripts/verify/run-scenario.mjs", "SFI-R12-S01"]],
      expect: /scenario SFI-R12-S01: evidence not observed in command output: rust 'infrastructure::sqlite::tests::catalog_all_songs_view'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "scripts", "verify", "manifest.json"),
          '"target": "publish_never_overwrites_a_target_created_after_planning"',
          '"target": "infrastructure::sqlite::tests::catalog_all_songs_view"',
        );
        return {};
      },
    },
  ];
}

import { join } from "node:path";
