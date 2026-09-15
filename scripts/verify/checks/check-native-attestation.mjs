#!/usr/bin/env node
// Native-scenario attestation check (task 13.9 / 13.5).
//
// The 45 `tests/native/*.md` scenarios describe HUMAN matrix steps that must
// be executed on a real target platform by an operator and recorded with
// evidence. They cannot be automated on every host. This check enforces that
// the attestation record for a scenario exists and is non-empty and complete —
// i.e. the human step manifest was actually executed and the outcome recorded
// — so `pnpm verify:scenario -- <native-ID>` fails loudly until the operator
// has done the work and left evidence, rather than silently passing on an
// un-run manual matrix.
//
// Attestation files:
//   artifacts/native-attestations/<ID>.log
// When the check runs on a platform that CAN automate a native row (currently
// macOS) and the row is covered by a real automated command, that command is
// preferred (see the scenario mapping in manifest.json); this check is the
// fallback for rows whose core is genuinely a manual OS interaction.
//
// Usage: node scripts/verify/checks/check-native-attestation.mjs <ID>...
// (also invoked by `run-scenario.mjs` via the manifest `command`.)

// `new URL(".", import.meta.url)` already resolves to this file's directory
// (`scripts/verify/checks/`), so three `..` land on the repository root — no
// extra `dirname` (which would resolve one level short, to `Project/music`).
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

const REQUIRED_FIELDS = [
  "os", "versions", "desktop-env", "operator", "result", "evidence-path", "date",
];

function fail(msg) {
  process.stderr.write(`FAIL attestation: ${msg}\n`);
  process.exitCode = 1;
}

function check(id) {
  const log = resolve(ROOT, "artifacts", "native-attestations", `${id}.log`);
  if (!existsSync(log)) {
    fail(`${id}: no attestation record at artifacts/native-attestations/${id}.log — run the tests/native/${id}.md human steps on the target platform and record the outcome`);
    return false;
  }
  const text = readFileSync(log, "utf8");
  if (!text.trim()) {
    fail(`${id}: attestation record is empty`);
    return false;
  }
  for (const field of REQUIRED_FIELDS) {
    if (!new RegExp(`^${field}:`, "m").test(text)) {
      fail(`${id}: attestation record missing required field '${field}'`);
      return false;
    }
  }
  if (!/result:\s*(pass|pass\s*\(.*\)|ok|ok\s*\(.*\))/i.test(text)) {
    fail(`${id}: attestation record must state a 'result:' of pass/ok`);
    return false;
  }
  process.stdout.write(`ok: native attestation ${id} recorded and complete\n`);
  return true;
}

const ids = process.argv.slice(2);
if (!ids.length) {
  fail("no scenario id given (usage: check-native-attestation.mjs <ID>...)");
  process.exit(1);
}
let ok = true;
for (const id of ids) if (!check(id)) ok = false;
process.exit(ok ? 0 : 1);