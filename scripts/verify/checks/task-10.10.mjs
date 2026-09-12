#!/usr/bin/env node
// Task 10.10 check: multi-select import batch — per-file results (imported /
// duplicate / unsupported / failed / library-unavailable), summary counts and
// retry that never redoes the already-imported items. `choose_and_import_files`
// is service-layer (crates/echo-desktop/src/runtime/services.rs); its Tauri
// registration is composition-root wiring.
//
// "编号/歌词部分成功" semantics come from echo-core import results (5.x) and
// are surfaced per-file here; the UI is verified by the ImportBatchDialog tests.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const IMPORT_TEST = resolve(APP, "src/features/import/ImportBatchDialog.test.tsx");
if (!existsSync(IMPORT_TEST)) {
  process.stderr.write(`FAIL 10.10: missing ${IMPORT_TEST}\n`);
  process.exit(1);
}

const STEPS = [
  ["typecheck", ["--filter", "@echo/desktop", "typecheck"]],
  [
    "import-tests",
    ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/import/ImportBatchDialog.test.tsx"],
  ],
];

for (const [name, args] of STEPS) {
  const r = spawnSync("pnpm", args, { cwd: APP, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 10.10: '${name}' exited ${r.status}\n${r.stderr}\n`);
    process.exit(1);
  }
}

process.stdout.write("ok 10.10: multi-select import per-file results, summary and retry verified\n");
