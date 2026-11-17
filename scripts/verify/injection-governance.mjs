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
      expect: /FAIL 1\.5: ci\.yml rust matrix is missing entry 'Linux\/workspace'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "          - { platform: Linux, os: ubuntu-latest, shard: workspace, cache: linux-workspace, packages: \"--workspace\" }\n",
          "          - { platform: Darwin, os: ubuntu-latest, shard: workspace, cache: linux-workspace, packages: \"--workspace\" }\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/test-condition-windows",
      guard: "the Linux workspace Test step being gated to a platform-specific condition, so PRs stop running the full suite",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: rust Test must not be conditionally skipped/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Test\n        run: cargo test ${{ matrix.packages }} --all-targets --all-features\n",
          "      - name: Test\n        if: runner.os == 'Windows'\n        run: cargo test ${{ matrix.packages }} --all-targets --all-features\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/rust-static-step-missing",
      guard: "the Rust Linux job losing its rust-static governance step",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'rust' is missing step 'Rust static gates'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Rust static gates\n        run: pnpm verify:governance -- --sections=rust-static\n\n",
          "",
        );
        return {};
      },
    },
    {
      id: "ci-structure/doc-skip-gate-input",
      guard: "a gate input (README.md, read by tasks 17.1/18.1) being added to the doc-only paths-ignore whitelist, which would skip CI on a PR that changes a gated file",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml push\.paths-ignore must be exactly/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - docs/arch-standards-remaining.md\n",
          "      - docs/arch-standards-remaining.md\n      - README.md\n",
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
          "      - name: Test\n        run: cargo test ${{ matrix.packages }} --all-targets --all-features\n",
          "      - name: Test\n        continue-on-error: true\n        run: cargo test ${{ matrix.packages }} --all-targets --all-features\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/governance-static-run-disabled",
      guard: "governance-static keeping the section flags but replacing the run with an echo",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static run must be 'pnpm verify:governance -- --sections=registry,static'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "        run: pnpm verify:governance -- --sections=registry,static",
          '        run: echo "--sections=registry,static"',
        );
        return {};
      },
    },
    {
      id: "ci-structure/perf-step-dropped",
      guard: "the performance-scenarios step disappearing from task-gates core, leaving three delegated scenarios with no execution location",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'task-gates' is missing step 'Run performance scenarios'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Run performance scenarios\n        run: pnpm verify:perf\n",
          "",
        );
        return {};
      },
    },
    {
      id: "ci-structure/perf-step-narrowed",
      guard: "the performance-scenarios step being gated off pull requests, so the PR tier loses the perf gate",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: task-gates performance step must not be conditionally skipped/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Run performance scenarios\n        run: pnpm verify:perf",
          "      - name: Run performance scenarios\n        if: github.event_name != 'pull_request'\n        run: pnpm verify:perf",
        );
        return {};
      },
    },
    {
      id: "ci-structure/desktop-format-check-readded",
      guard: "a third job re-adding cargo fmt, which rust (Linux) and the main-only rust-desktop (macOS/Windows) already cover",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: 'cargo fmt --all -- --check' must run in exactly \['rust', 'rust-desktop'\] \(Linux \+ main-only macOS\/Windows\), got \[rust, rust-desktop, release-gate\]/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Run release gate without external artifacts\n",
          "      - name: Format check\n        run: cargo fmt --all -- --check\n\n      - name: Run release gate without external artifacts\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/desktop-clippy-missing",
      guard: "the main-only macOS/Windows job losing Clippy, so no job runs workspace-form clippy on those platforms",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'rust-desktop' is missing step 'Clippy'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Clippy\n        run: cargo clippy --workspace --all-targets --all-features -- -D warnings\n\n      # This job only exists on main",
          "      # This job only exists on main",
        );
        return {};
      },
    },
    {
      id: "ci-structure/frontend-matrix-widened",
      guard: "a macOS or Windows frontend leg being added back, re-running jsdom checks that never read process.platform",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml frontend matrix contains unexpected platform entries/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "        include:\n          - platform: Linux\n            os: ubuntu-latest",
          "        include:\n          - platform: Linux\n            os: ubuntu-latest\n          - platform: macOS\n            os: xcode-27",
        );
        return {};
      },
    },
    {
      id: "ci-structure/injection-job-readded",
      guard: "the injection proof suite being re-added to a CI job, re-spending three runner instances per PR on gate self-tests",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml must not run the injection proof suite in a job \(local-only\), found \[release-gate:Prove governance gates\]/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Run release gate without external artifacts\n",
          "      - name: Prove governance gates\n        run: node scripts/verify/injection-suite.mjs\n\n      - name: Run release gate without external artifacts\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/duplicate-player-smoke",
      guard: "task-13.2 re-adding the bare player_smoke run that silently skips without the staged dylibs",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: task-13\.2 must not run player_smoke bare \(task-8\.12 is the authoritative real-libmpv run\)/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "scripts", "verify", "checks", "task-13.2.mjs"),
          'run("cargo", ["test", "-p", "echo-desktop", "--all-features", "player::coordinator::tests::"]);\n',
          'run("cargo", ["test", "-p", "echo-desktop", "--all-features", "player::coordinator::tests::"]);\nrun("cargo", ["test", "-p", "echo-desktop", "--all-features", "--test", "player_smoke"]);\n',
        );
        return {};
      },
    },
    {
      id: "ci-structure/section-coverage-missing",
      guard: "the main-only coverage step keeping its name while running the static sections instead of coverage",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static coverage step run must be 'pnpm verify:governance -- --sections=coverage'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "        run: pnpm verify:governance -- --sections=coverage",
          "        run: pnpm verify:governance -- --sections=static",
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
      id: "ci-structure/shard-packages-drift",
      guard: "the Linux workspace shard's package selection narrowing away from --workspace, so the suite stops covering the workspace",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml rust matrix entry 'Linux\/workspace' maps packages to '-p echo-core', expected '--workspace'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "shard: workspace, cache: linux-workspace, packages: \"--workspace\"",
          "shard: workspace, cache: linux-workspace, packages: \"-p echo-core\"",
        );
        return {};
      },
    },
    {
      id: "ci-structure/clippy-moved-after-test",
      guard: "Clippy running after Test, which would leave a check-shape build as the cached artifact",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: rust step order must be Format check < Clippy < Test < Rust static gates/,
      baseline: true,
      inject() {
        const clippyBlock = "      - name: Clippy\n        run: cargo clippy --workspace --all-targets --all-features -- -D warnings\n\n";
        replaceIn(join(ROOT, ".github", "workflows", "ci.yml"), clippyBlock, "");
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "        run: cargo test ${{ matrix.packages }} --all-targets --all-features\n",
          `        run: cargo test \${{ matrix.packages }} --all-targets --all-features\n\n${clippyBlock}`,
        );
        return {};
      },
    },
    {
      id: "ci-structure/purity-section-removed",
      guard: "governance-static keeping the purity step name while dropping the purity section",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static purity step run must be 'pnpm verify:governance -- --sections=purity'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "        run: pnpm verify:governance -- --sections=purity",
          "        run: pnpm verify:governance -- --sections=static",
        );
        return {};
      },
    },
    {
      id: "ci-structure/coverage-gated-off-main",
      guard: "the coverage step losing its main-only skip condition, so it would either run on PRs or be permanently skipped",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static 'Run coverage gate' if must be 'github\.event_name != 'pull_request'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Run coverage gate\n        if: github.event_name != 'pull_request'\n",
          "      - name: Run coverage gate\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/audit-offline-tiering-frozen",
      guard: "the dependency audit's PR tiering pinned to a constant, which would silently disable the online scan on every event",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static audit step env ECHO_AUDIT_OFFLINE_ONLY must be/,
      baseline: true,
      inject() {
        // ```replaceIn``` is first-occurrence-only, and the same env line appears
        // twice in ci.yml — once under Install audit tooling, once under Verify
        // dependency audit gate. A bare env needle would rewrite the installer and
        // leave the step task-1.5 actually asserts on untouched, so the proof would
        // pass for the wrong reason. Carry the step name to make it unique.
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Verify dependency audit gate\n        env:\n          ECHO_AUDIT_OFFLINE_ONLY: ${{ github.event_name == 'pull_request' && '1' || '0' }}\n",
          '      - name: Verify dependency audit gate\n        env:\n          ECHO_AUDIT_OFFLINE_ONLY: "1"\n',
        );
        return {};
      },
    },
    {
      id: "ci-structure/desktop-shard-gated-to-main",
      guard: "the macOS/Windows desktop shards gated so their workspace tests never run on main either",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: rust-desktop job if must be 'github\.event_name != 'pull_request'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "  rust-desktop:\n    name: Rust desktop (${{ matrix.platform }})\n    runs-on: ${{ matrix.os }}\n    if: github.event_name != 'pull_request'\n",
          "  rust-desktop:\n    name: Rust desktop (${{ matrix.platform }})\n    runs-on: ${{ matrix.os }}\n    if: github.event_name == 'push'\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/job-if-references-matrix",
      guard: "a job-level if reaching for the matrix context, which GitHub evaluates before the matrix exists and rejects the whole workflow for",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'rust-desktop' if must not reference the matrix context/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "    if: github.event_name != 'pull_request'\n",
          "    if: github.event_name != 'pull_request' || matrix.shard != 'desktop'\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/rust-job-gated-off-pr",
      guard: "the Rust matrix job (Linux workspace tests, 16.1, 9.7/9.8, rust-static) gated so it stops running entirely",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'rust' must not be conditionally skipped/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "  rust:\n    name: Rust (${{ matrix.platform }} ${{ matrix.shard }})\n    runs-on: ${{ matrix.os }}\n",
          "  rust:\n    name: Rust (${{ matrix.platform }} ${{ matrix.shard }})\n    runs-on: ${{ matrix.os }}\n    if: github.event_name != 'pull_request'\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/audit-step-if-false",
      guard: "the dependency audit step switched off with the YAML boolean `if: false`, which an emptiness check folds into 'unset'",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: governance-static audit step must not be conditionally skipped/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Verify dependency audit gate\n",
          "      - name: Verify dependency audit gate\n        if: false\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/cargo-shape-drift-after-test",
      guard: "a cargo call of a different shape added after the cache-shape Test step, which would leave the saved cache mismatched with its readers",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'rust' step 'Extra lib tests' runs .* after the cache-shape Test step/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Rust static gates\n        run: pnpm verify:governance -- --sections=rust-static\n",
          "      - name: Rust static gates\n        run: pnpm verify:governance -- --sections=rust-static\n\n      - name: Extra lib tests\n        run: cargo test --workspace --lib --all-features\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/native-gate-payload-removed",
      guard: "the macOS native Gate's only real step emptied, leaving a permanently-green job that no longer runs 8.12/13.2",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: macos-native-gate run must be 'pnpm verify:task -- 8\.12 13\.2'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Verify libmpv smoke and native e2e gates\n        run: pnpm verify:task -- 8.12 13.2\n",
          "      - name: Verify libmpv smoke and native e2e gates\n        run: echo skipped\n",
        );
        return {};
      },
    },
    {
      id: "ci-structure/embedded-step-missing",
      guard: "the macOS platform Gate losing the embedded-frontend freshness check, leaving the `embedded` section with no execution location",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'macos-platform-gate' is missing step 'Verify embedded frontend freshness'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Verify embedded frontend freshness\n        run: node scripts/verify/checks/check-embedded-frontend.mjs --bin target/aarch64-apple-darwin/release/bundle/macos/Echo.app/Contents/MacOS/Echo\n\n",
          "",
        );
        return {};
      },
    },
    {
      id: "ci-structure/task-161-step-missing",
      guard: "governance-static losing the 16.1 static-contract step, whose only remaining carrier would be the workspace test's compile/behavior half",
      check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
      expect: /FAIL 1\.5: ci\.yml job 'governance-static' is missing step 'Task 16\.1 static contract'/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, ".github", "workflows", "ci.yml"),
          "      - name: Task 16.1 static contract\n        run: pnpm verify:task -- 16.1\n\n",
          "",
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
        // downstream as raw URL strings (`.url.to_string()`) instead of
        // decoded filesystem paths. The macOS Opened branch now delivers one
        // batch, so inject at the `deliver_file_opens` argument. The variable
        // is deliberately spelled `url` so the check's `url.to_string()` guard
        // trips on every runner (non-macOS exempts the dead-code lint on
        // `open_targets`, so clippy alone cannot prove the violation there).
        replaceIn(
          join(ROOT, "apps", "desktop", "src-tauri", "src", "bootstrap.rs"),
          "deliver_file_opens(app, &startup, open_targets::open_targets(&urls));",
          "deliver_file_opens(app, &startup, urls.iter().map(|url| url.to_string()).map(std::path::PathBuf::from).collect());",
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
      id: "licensing/stale-dependency-inventory",
      guard: "a lockfile-generated third-party inventory drifting without regeneration (issue #144: LICENSES.md used to list only 43 of 594 crates and miss two production npm dependencies)",
      check: ["node", ["scripts/verify/checks/task-18.1.mjs"]],
      expect: /FAIL 18\.1: [\s\S]*are stale/,
      baseline: true,
      inject() {
        replaceIn(
          join(ROOT, "LICENSES.md"),
          "# Echo — Third-Party Notices & License Inventory",
          "# Echo — Third-Party Notices & License Inventory (stale probe)",
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
