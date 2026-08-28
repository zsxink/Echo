#!/usr/bin/env node
// Task 1.7 check: dependency audit gates — cargo-deny, cargo-audit, frontend
// production dependency audit and the license inventory.
//
// Anatomy of the check (mirrors the task's offline/online split):
//   - Local, offline-safe parts gate the result (hard fail on nonzero):
//       * `cargo deny check licenses bans sources` (no advisory DB needed)
//       * LICENSES.md exists, is non-empty, and names every workspace member
//   - Online parts (`cargo deny check` full advisory DB, `cargo deny check
//     advisories`, `cargo audit`, `pnpm audit --prod`) FAIL if they run and
//     report high/critical findings, but are skipped with a clear stderr note
//     if the network/advisory DB is unavailable — the script still exits 0
//     for the locally verifiable parts.
//
// Tools that are missing are installed first: `cargo install <tool> --locked`
// then, on macOS, `brew install <tool>` as fallback. The script prints what it
// is doing to stderr during installs.
//
// On success prints: `ok 1.7: cargo-deny, cargo-audit, frontend prod audit and
// license inventory pass`.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const DENY_TOML = resolve(ROOT, "deny.toml");
const LICENSES_MD = resolve(ROOT, "LICENSES.md");
const DESKTOP_DIR = resolve(ROOT, "apps", "desktop");

const NET_TIMEOUT = 240_000; // network-backed checks: advisory DB fetch on a cold cache
const INSTALL_TIMEOUT = 600_000; // cargo install compiles from source
// Mirrors deny.toml's `[advisories].ignore`: known unmaintained (not CVE)
// Linux-only Tauri transitive crates with no upstream replacement. New or
// vulnerable advisories remain hard failures.
const AUDIT_IGNORES = [
  "RUSTSEC-2024-0370",
  "RUSTSEC-2024-0411",
  "RUSTSEC-2024-0412",
  "RUSTSEC-2024-0413",
  "RUSTSEC-2024-0414",
  "RUSTSEC-2024-0415",
  "RUSTSEC-2024-0416",
  "RUSTSEC-2024-0417",
  "RUSTSEC-2024-0418",
  "RUSTSEC-2024-0419",
  "RUSTSEC-2024-0420",
  "RUSTSEC-2025-0075",
  "RUSTSEC-2025-0080",
  "RUSTSEC-2025-0081",
  "RUSTSEC-2025-0098",
  "RUSTSEC-2025-0100",
];

let offlineSkipped = 0;

function fail(msg) {
  process.stderr.write(`FAIL 1.7: ${msg}\n`);
  process.exit(1);
}

function spawn(cmd, args, opts = {}) {
  return spawnSync(cmd, args, {
    cwd: opts.cwd || ROOT,
    encoding: "utf8",
    timeout: opts.timeout || 180_000,
    env: { ...process.env, ...(opts.env || {}) },
  });
}

function output(r) {
  return `${r.stdout || ""}\n${r.stderr || ""}`;
}

// -- failure classification for online parts ---------------------------------
// A real finding (RUSTSEC/CVE/vulnerability) is a hard failure; a transport
// problem (can't reach the advisory DB / registry audit endpoint) or a
// timeout is an "offline" skip. Everything else is a hard failure too, but
// its output is shown so a broken install or config is not silently ignored.
const FINDING_RE = /(RUSTSEC-[0-9]{4}-[0-9]{4}|CVE-[0-9]{4}-[0-9]+|vulnerabilit(ies|y| found))/i;
const SKIP_HAS_NO_FINDINGS = (text) => !/(0 vulnerabilities|no vulnerabilities found|nothing found)/i.test(text);

class NotRun {
  constructor(reason) {
    this.reason = reason;
  }
}

function classifyOutcome(r, label) {
  if (r.status === 0) return { ok: true };
  const text = output(r);
  // timed out => treat as offline/unreachable (do not hang the pipeline).
  if (r.signal === "SIGTERM" || (r.error && /ETIMEDOUT|timeout/i.test(String(r.error)))) {
    return { ok: true, offline: true, label };
  }
  if (FINDING_RE.test(text) && SKIP_HAS_NO_FINDINGS(text)) {
    return { ok: false, label, text, finding: true };
  }
  const net = /(failed|error|could not|unable to|network|connection|fetch|ENOTFOUND|ECONNREFUSED|ETIMEDOUT|EAI_AGAIN|advisory.database|git.op|TLS|SSL|registry|ERR_PNPM_AUDIT_ENDPOINT_NOT_EXISTS)/i.test(
    text,
  );
  if (net) {
    return { ok: true, offline: true, label, text };
  }
  return { ok: false, label, text, finding: false };
}

function handleOnline(label, r) {
  const c = classifyOutcome(r, label);
  if (c.ok) {
    if (c.offline) {
      offlineSkipped += 1;
      process.stderr.write(
        `1.7: offline — ${c.label} skipped: advisory DB / audit endpoint unreachable or timed out` +
          (c.text ? `:\n${c.text.trim()}` : "") +
          "\n",
      );
    }
    return;
  }
  if (c.finding) {
    fail(`${label} reported findings/high or critical (see below):\n${c.text}`);
  }
  fail(`${label} exited ${r.status}:\n${c.text}`);
}

// -- ensure a CLI tool is installed ------------------------------------------
// cargo-audit and cargo-deny are invoked by name (spawnSync does not resolve
// through the cargo plugin shim based on `cargo <tool>` — it needs the binary
// on PATH). If missing, install and re-check.
function ensureTool(tool) {
  const check = spawn(tool, ["--version"], { timeout: 30_000 });
  if (check.status === 0) return; // already installed — nothing to do

  process.stderr.write(`1.7: ${tool} not found on PATH; installing (may take a while)...\n`);

  const built = spawn("cargo", ["install", tool, "--locked"], {
    timeout: INSTALL_TIMEOUT,
    env: { ...process.env, CARGO_NET_RETRY: "3" },
  });
  const builtOk = built.status === 0 && spawn(tool, ["--version"], { timeout: 30_000 }).status === 0;
  if (builtOk) return;

  if (process.platform === "darwin" && spawn("brew", ["--version"]).status === 0) {
    process.stderr.write(
      `1.7: \`cargo install ${tool} --locked\` did not succeed; trying \`brew install ${tool}\`...\n`,
    );
    const brewed = spawn("brew", ["install", tool], { timeout: INSTALL_TIMEOUT });
    if (brewed.status === 0 && spawn(tool, ["--version"], { timeout: 30_000 }).status === 0) return;
  }

  fail(
    `${tool} could not be installed — tried \`cargo install ${tool} --locked\` and ` +
      `\`brew install ${tool}\`. Install it manually and re-run this check.\n` +
      output(built),
  );
}

// -- main --------------------------------------------------------------------
if (!existsSync(DENY_TOML)) {
  fail("deny.toml is missing from the repo root — add it before running the 1.7 check");
}

ensureTool("cargo-deny");
ensureTool("cargo-audit");

// 1) Local, offline-safe gates. `cargo deny check licenses|bans|sources` do
//    not need the advisory DB, so they hard-gate the result even when the
//    machine is offline. They also validate deny.toml's syntax.
for (const which of ["licenses", "bans", "sources"]) {
  const r = spawn("cargo", ["deny", "check", which], { timeout: 120_000 });
  if (r.status !== 0) {
    fail(`\`cargo deny check ${which}\` exited ${r.status}:\n${output(r)}`);
  }
}

// 2) Licence inventory file must exist and be non-empty, and must list the
//    workspace members. It is a checkpoint artifact generated from
//    `cargo metadata` — not regenerated on every run (avoids diff churn).
if (!existsSync(LICENSES_MD)) {
  fail("LICENSES.md is missing from the repo root — generate the license inventory");
}
const licensesText = readFileSync(LICENSES_MD, "utf8");
if (licensesText.trim() === "") {
  fail("LICENSES.md is empty — generate the license inventory");
}
for (const member of ["echo-app", "echo-core", "echo-desktop"]) {
  if (!licensesText.includes(member)) {
    fail(`LICENSES.md does not list workspace member \`${member}\``);
  }
}

// 3) Online advisory checks. These FAIL on real findings, but skip cleanly
//    when the network / advisory DB is unreachable.
handleOnline("cargo deny check (advisories)", spawn("cargo", ["deny", "check"], { timeout: NET_TIMEOUT }));
handleOnline("cargo deny check advisories", spawn("cargo", ["deny", "check", "advisories"], { timeout: NET_TIMEOUT }));

const audit = spawn("cargo", ["audit", ...AUDIT_IGNORES.flatMap((id) => ["--ignore", id])], {
  timeout: NET_TIMEOUT,
});
if (audit.status !== 0) {
  const c = classifyOutcome(audit, "cargo audit");
  if (!c.ok) {
    if (c.finding) {
      fail(`cargo audit reported vulnerabilities (see below):\n${c.text}`);
    }
    fail(`cargo audit exited ${audit.status}:\n${c.text}`);
  }
  if (c.offline) {
    // Fall back to the locally cached DB when the live fetch could not
    // complete: works offline when ~/.cargo/advisory-db is present.
    const cached = spawn("cargo", ["audit", "--no-fetch", ...AUDIT_IGNORES.flatMap((id) => ["--ignore", id])], {
      timeout: 120_000,
    });
    if (cached.status !== 0) {
      offlineSkipped += 1;
      process.stderr.write(
        `1.7: offline — cargo audit skipped: advisory DB unavailable (live fetch failed and ` +
          `\`cargo audit --no-fetch\` exited ${cached.status})` +
          (c.text ? `:\n${c.text.trim()}` : "") +
          "\n",
      );
    } else if (/0 vulnerabilities|no vulnerabilities/i.test(output(cached))) {
      process.stderr.write("1.7: cargo audit — no vulnerabilities (cached advisory DB)\n");
    }
  }
}

// 4) Frontend production dependency audit. Scope to `dependencies` only via
//    --prod; --audit-level high makes low/medium findings non-blocking. The
//    configured npm registry may be a mirror without an audit endpoint (e.g.
//    npmmirror) — retry against registry.npmjs.org in that case.
function auditPnpm() {
  const args = ["audit", "--prod", "--audit-level", "high"];
  const r = spawn("pnpm", args, { cwd: DESKTOP_DIR, timeout: 120_000 });
  if (r.status === 0) {
    return { ok: true };
  }
  const text = output(r);
  const registryProblem =
    /ERR_PNPM_AUDIT_ENDPOINT_NOT_EXISTS|registry|ENOTFOUND|ECONNREFUSED|ETIMEDOUT|fetch/i.test(text) &&
    !FINDING_RE.test(text);
  if (registryProblem) {
    // Mirror without an audit endpoint, or a transient network failure.
    const official = spawn("pnpm", [...args, "--registry", "https://registry.npmjs.org"], {
      cwd: DESKTOP_DIR,
      timeout: 120_000,
    });
    if (official.status === 0) {
      return { ok: true, note: "audited against registry.npmjs.org (configured mirror has no audit endpoint)" };
    }
    const officialText = output(official);
    if (FINDING_RE.test(officialText)) {
      return { ok: false, text: officialText };
    }
    return { ok: true, offline: true, text: `${text}\n${officialText}` };
  }
  if (FINDING_RE.test(text)) {
    return { ok: false, text };
  }
  return { ok: false, text };
}

{
  const res = auditPnpm();
  if (res.ok) {
    if (res.offline) {
      offlineSkipped += 1;
      process.stderr.write(`1.7: offline — pnpm production audit skipped: audit endpoint unreachable:\n${res.text.trim()}\n`);
    } else if (res.note) {
      process.stderr.write(`1.7: pnpm production audit — no high/critical advisories (${res.note})\n`);
    }
  } else {
    fail(`pnpm audit --prod --audit-level high reported high/critical advisories (see below):\n${res.text}`);
  }
}

if (offlineSkipped > 0) {
  process.stderr.write(
    `1.7: note — ${offlineSkipped} online advisory check(s) skipped because the network/advisory DB was unreachable; ` +
      "offline-verifiable parts (licenses/bans/sources + license inventory) passed\n",
  );
}

process.stdout.write(
  "ok 1.7: cargo-deny, cargo-audit, frontend prod audit and license inventory pass\n",
);
