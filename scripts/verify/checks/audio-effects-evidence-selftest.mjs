#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runtimeEvidenceProblems, taskIdProblems } from "./audio-effects-evidence-rules.mjs";
try {
  const evidence = JSON.parse(readFileSync(new URL("../../../openspec/changes/introduce-audio-effects-equalizer/evidence/macos-runtime-parameter-recheck-20261008.json", import.meta.url), "utf8"));
  assert.deepEqual(runtimeEvidenceProblems(evidence), []);
  for (const mutation of [
    (copy) => { delete copy.checks[0].command_result; },
    (copy) => { copy.checks[0].measured_change_db = 0; },
    (copy) => { copy.checks[0].changed_window_frames[0] = copy.checks[0].command_frame_upper_bound; },
    (copy) => { copy.checks[0].changed_window_frames[1] = copy.checks[0].output_frames + 1; },
    (copy) => { copy.checks[0].eof_before_command = true; },
    (copy) => { copy.checks.find((item) => item.name === "static_gain_reference").error_db = 1; },
    (copy) => { copy.gate_passed = true; },
  ]) {
    const copy = structuredClone(evidence);
    mutation(copy);
    assert.ok(runtimeEvidenceProblems(copy).length, "invalid evidence escaped the gate");
  }
  const manifestIds = new Set(["1.2", "AFX-12.3"]);
  assert.deepEqual(taskIdProblems(["AFX-1.2", "AFX-12.3"], manifestIds), []);
  assert.ok(taskIdProblems(["1.2"], manifestIds).length, "bare collision escaped");
  assert.ok(taskIdProblems(["AFX-12.3", "AFX-12.3"], manifestIds).length, "qualified duplicate escaped");
  console.log("ok: evidence gate rejects inconsistent PCM, missing facts, duplicate tasks and bare collisions");
} catch (error) {
  console.error(error);
  process.exitCode = 1;
}
