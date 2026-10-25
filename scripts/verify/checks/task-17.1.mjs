#!/usr/bin/env node
// Windows installer contract gate (task 17.1).
//
// v0.3.0's `Echo_0.3.0_x64-setup.exe` could not install on Windows 10
// machines without a preinstalled WebView2 runtime (issue #64). The root
// cause was not a broken artifact but an *absent* one: with no
// `bundle.windows` section every Tauri default applied at once —
// `downloadBootstrapper` (the NSIS installer downloads the WebView2
// bootstrapper at install time through NSISdl, a plaintext-HTTP-only plugin),
// `languages: ["English"]` (a Chinese Windows 10 user gets an English error
// they cannot read, and reports "cannot install"), and no minimum-OS gate at
// all (the Win10 22H2 baseline in docs/ROADMAP.md existed only on paper).
//
// The fix lives entirely in `tauri.windows.conf.json` plus one NSIS hook
// file. None of that is executable code, so nothing fails at compile time if
// someone later "simplifies" the config back to the defaults — the installer
// would quietly regress to the exact behavior users reported. This gate makes
// that regression loud: it re-derives the merged Windows bundle config the
// way Tauri merges platform overrides and asserts the whole contract.
//
// Usage:
//   node scripts/verify/checks/task-17.1.mjs
//
// Exits 0 when every contract item holds, 1 naming the first violated item
// (all violations found are printed; the first is the headline).

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const SRC_TAURI = resolve(ROOT, "apps", "desktop", "src-tauri");
const BASE_CONFIG = resolve(SRC_TAURI, "tauri.conf.json");
const WINDOWS_CONFIG = resolve(SRC_TAURI, "tauri.windows.conf.json");
const README_ZH = resolve(ROOT, "README.md");
const README_EN = resolve(ROOT, "README.en.md");

const EXPECTED_LANGUAGES = ["SimpChinese", "English"];

/** Tauri merges a platform config over the base config recursively; plain
 *  object values merge key by key, everything else replaces. The base has no
 *  `bundle.windows` (asserted below), so in practice the override's subtree
 *  is what ships — but compute it honestly instead of assuming. */
function deepMerge(base, override) {
  const out = { ...base };
  for (const [key, value] of Object.entries(override)) {
    out[key] =
      value && typeof value === "object" && !Array.isArray(value) && base?.[key] && typeof base[key] === "object"
        ? deepMerge(base[key], value)
        : value;
  }
  return out;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

const violations = [];
const check = (ok, message) => {
  if (!ok) violations.push(message);
};

const base = readJson(BASE_CONFIG);
const override = readJson(WINDOWS_CONFIG);
const merged = deepMerge(base, override);
const windows = merged.bundle?.windows;

// --- Contract 1: WebView2 fallback is the embedded bootstrapper -------------
check(
  windows?.webviewInstallMode?.type === "embedBootstrapper",
  "bundle.windows.webviewInstallMode.type must be \"embedBootstrapper\" — the Tauri default (downloadBootstrapper) installs through the plaintext-HTTP NSISdl path that fails on restricted networks (issue #64)",
);
check(
  windows?.webviewInstallMode?.silent === true,
  "bundle.windows.webviewInstallMode.silent must be true (silent bootstrapper run)",
);

// --- Contract 2: a declared WebView2 floor triggers install-time updates ----
check(
  typeof windows?.minimumWebview2Version === "string" && windows.minimumWebview2Version.trim().length > 0,
  "bundle.windows.minimumWebview2Version must be a non-empty version string — without it a stale runtime is only discovered at app launch (blank window)",
);

// --- Contract 3: bilingual installer, Chinese as the fallback language ------
const languages = windows?.nsis?.languages;
check(
  Array.isArray(languages) && languages.length === EXPECTED_LANGUAGES.length && languages.every((l, i) => l === EXPECTED_LANGUAGES[i]),
  `bundle.windows.nsis.languages must be exactly ${EXPECTED_LANGUAGES.join(", ")} in this order — the first language is the fallback when the OS language is not in the list, and dropping it reverts the installer to English-only (issue #64)`,
);

// --- Contract 4: the Win10 floor hook exists and still gates ----------------
const hooksRelative = windows?.nsis?.installerHooks;
check(typeof hooksRelative === "string" && hooksRelative.length > 0, "bundle.windows.nsis.installerHooks must name the hooks file");
if (typeof hooksRelative === "string" && hooksRelative) {
  const hooksPath = resolve(SRC_TAURI, hooksRelative);
  check(existsSync(hooksPath), `installerHooks file not found: ${hooksPath}`);
  if (existsSync(hooksPath)) {
    const hooks = readFileSync(hooksPath, "utf8");
    check(hooks.includes("NSIS_HOOK_PREINSTALL"), `${hooksRelative} must define NSIS_HOOK_PREINSTALL — that is the hook macro the Tauri template injects before copying files`);
    check(hooks.includes("AtLeastWin10"), `${hooksRelative} must gate on \${AtLeastWin10} (WinVer.nsh) — the minimum-OS floor below Windows 10`);
  }
}

// --- Contract 5: platform neutrality of the base config ---------------------
check(
  !(base.bundle && base.bundle.windows),
  "tauri.conf.json must stay platform-neutral: bundle.windows belongs in tauri.windows.conf.json, not in the base config",
);

// --- Contract 6: the documented requirement is actually documented ----------
for (const [path, label] of [
  [README_ZH, "README.md"],
  [README_EN, "README.en.md"],
]) {
  const text = readFileSync(path, "utf8");
  check(text.includes("Windows 10"), `${label} must state the Windows minimum (Windows 10)`);
  check(text.includes("x64"), `${label} must state that Windows builds are x64-only`);
}

if (violations.length) {
  process.stderr.write(`FAIL windows-installer: ${violations[0]}\n`);
  for (const extra of violations.slice(1)) {
    process.stderr.write(`  also: ${extra}\n`);
  }
  process.exit(1);
}

process.stdout.write(
  "ok windows-installer: embedBootstrapper + SimpChinese/English + minimumWebview2Version + Win10 hook + README requirements all hold\n",
);
