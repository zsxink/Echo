#!/usr/bin/env node
// Task AFX-1: Guard the audio-effects evidence discipline.
//
// This change carries a class of defect that the rest of the repository's gates
// do not catch, and that has already produced false greens here:
//
//   1. Task numbers inside an OpenSpec change are LOCAL to that change, but
//      `pnpm verify:task -- <id>` resolves against `scripts/verify/manifest.json`.
//      Several audio-effects task numbers (1.1, 1.2, 2.2, 4.5, 4.7, 4.8, 6.1,
//      6.2) already exist there with completely different meanings, so running
//      the gate for them executes unrelated tests and reports success. A
//      collision therefore presents as GREEN while verifying nothing about
//      audio effects — strictly worse than a missing gate.
//
//   2. The native probe tools are not registered anywhere in the manifest, so
//      "the probe passed" has no gate behind it.
//
//   3. `af-command` returns rc=0 even when the parameter does not change, so any
//      claim of the form "the runtime command succeeded, therefore the filter
//      changed" is unproven. It does not follow that runtime parameter changes
//      are ineffective: the earlier PCM experiment that concluded so used the
//      wrong mpv label and has been retracted (AFX-9.7). Whether they reach the
//      audio is still open (AFX-9.5), so the check asserts only the narrower
//      claim — rc is not evidence either way.
//
// Checks performed:
//   A. Report every audio-effects task number that collides with a manifest id.
//   B. Require the change's own task list to reference the corrected evidence
//      (the PCM discriminating-measurement probe) rather than the discredited
//      structural argument.
//   C. Require native-gate.md to state the measured runtime-parameter result, so
//      the corrected finding cannot be quietly reverted.
//   D. Require probe.py to actually be able to prove a runtime change, i.e. the
//      discriminating flag exists and the shipped tool still self-tests.

import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const CHANGE = "openspec/changes/introduce-audio-effects-equalizer";
const failures = [];
const notes = [];

const read = (relative) => readFileSync(resolve(ROOT, relative), "utf8");

// --- A. task-number collisions with the shared manifest -----------------------
const manifest = JSON.parse(read("scripts/verify/manifest.json"));
const manifestIds = new Set((manifest.tasks || []).map((task) => task.id));

if (!existsSync(resolve(ROOT, `${CHANGE}/tasks.md`))) {
  failures.push(`missing ${CHANGE}/tasks.md`);
} else {
  const tasks = read(`${CHANGE}/tasks.md`);
  // Accept both bare (1.2) and qualified (AFX-1.2) numbering; the collision
  // check compares whatever id appears against the manifest.
  const ids = [...tasks.matchAll(/^- \[([ x])\] ((?:AFX-)?\d+(?:\.\d+)+)/gm)].map((m) => m[2]);
  if (!ids.length) {
    failures.push(`no task entries parsed from ${CHANGE}/tasks.md; the collision guard is blind`);
  }
  const bare = [...new Set(ids)].filter((id) => manifestIds.has(id));
  const qualified = [...new Set(ids)].filter((id) => /^AFX-/.test(id));
  if (bare.length) {
    failures.push(
      `audio-effects task numbers collide with scripts/verify/manifest.json: ` +
        `${bare.join(", ")}. "pnpm verify:task -- <id>" would run the manifest's ` +
        `unrelated task and report success, so those checked boxes are green ` +
        `without verifying audio effects. Prefix them (e.g. AFX-4.8).`,
    );
  } else if (qualified.length) {
    notes.push(`${qualified.length} qualified task ids, no manifest collision`);
  }

  // --- B. the discredited structural argument must not be the justification ---
  if (/随包库结构检查.{0,40}已证明.{0,20}process_command\s*为空/.test(tasks)) {
    failures.push(
      "tasks.md still justifies 4.5 by claiming the packaged library has no " +
        "process_command. That was disproved (the callback exists; the struct " +
        "reader had the wrong field order). Cite the PCM measurement instead.",
    );
  }
  if (!/runtime-parameter-proof/.test(tasks)) {
    failures.push(
      "tasks.md never references probe.py --runtime-parameter-proof, the only " +
        "measurement that can distinguish a real parameter change from rc=0.",
    );
  }
}

// --- C. native-gate must carry the measured runtime result -------------------
if (!existsSync(resolve(ROOT, `${CHANGE}/native-gate.md`))) {
  failures.push(`missing ${CHANGE}/native-gate.md`);
} else {
  const gate = read(`${CHANGE}/native-gate.md`);
  if (!/runtime_parameter_pcm_proof|runtime-parameter-proof/.test(gate)) {
    failures.push(
      "native-gate.md does not document the runtime-parameter PCM measurement, " +
        "so the corrected finding (runtime commands return rc=0 but do not " +
        "change the audio) is unrecorded and may be reverted by mistake.",
    );
  }
  if (/spatial:m`[^|\n]*静默无效|`extrastereo`[^|\n]*\*\*没有\*\*\s*\|\s*❌/.test(gate)) {
    failures.push(
      "native-gate.md still states extrastereo has no process_command. That " +
        "claim was disproved; the callback exists.",
    );
  }
  // `latency=true` compensates delay; asserting it adds delay inverts the fact.
  if (/latency=true[^。\n]{0,40}(引入|多出|额外)[^。\n]{0,10}延迟/.test(gate)) {
    failures.push(
      "native-gate.md describes alimiter latency=true as adding delay. Measured " +
        "behaviour is the opposite: it compensates the lookahead delay by " +
        "trimming the head and padding the tail (239 samples at 48 kHz/5 ms).",
    );
  }
  // The 2026-10-03 conclusion "runtime parameter changes never take effect" was
  // withdrawn on 2026-10-04: its PCM experiment used the wrong mpv label, so the
  // whole discriminating chain was invalid. Catch a live assertion of it.
  // Scope the retraction marker to the SAME markdown table row / bullet, since a
  // wider window picks up unrelated retraction wording from a previous section.
  const RETRACTED = /撤回|作废|已证伪|不再成立/;
  let assertedIneffective = false;
  for (const line of gate.split("\n")) {
    if (!/运行时改参一律无效|运行时改参整体失效/.test(line)) continue;
    if (!RETRACTED.test(line)) assertedIneffective = true;
  }
  if (assertedIneffective) {
    failures.push(
      "native-gate.md asserts runtime parameter changes are categorically " +
        "ineffective. That conclusion was withdrawn on 2026-10-04 because the PCM " +
        "experiment used the wrong mpv label (the `af` property reports the filter " +
        "NAME, not the label). The defensible claim is narrower: rc is not " +
        "evidence, and runtime updates are NOT YET PROVEN to work.",
    );
  }
  if (!/AFX-9\.5|RUNTIME_PARAM/.test(gate)) {
    failures.push(
      "native-gate.md does not record the open root cause (AFX-9.5) or the " +
        "RUNTIME_PARAM return-code signature, so the withdrawn claim may be " +
        "reinstated by mistake.",
    );
  }
}

// --- D. the probe must still be able to prove a runtime change --------------
const probe = resolve(ROOT, "scripts/audio-effects/probe.py");
if (!existsSync(probe)) {
  failures.push("missing scripts/audio-effects/probe.py");
} else {
  const source = readFileSync(probe, "utf8");
  for (const [needle, why] of [
    ["--runtime-parameter-proof", "cannot prove a runtime parameter change at all"],
    ["--runtime-capture-seconds", "has no fixed capture window, so results depend on decode timing"],
    ["spatial_width_capture", "has no width A/B measurement"],
  ]) {
    if (!source.includes(needle)) {
      failures.push(`probe.py lacks ${needle}: it ${why}`);
    }
  }
}

const inspector = read("scripts/audio-effects/inspect-filter-runtime.py");
// FFmpeg 6.0 declares `int priv_size` (not size_t); what matters is that
// inputs/outputs precede priv_class, the pad counts are uint8, and priv_size
// sits late in the struct. The original bug was the field ORDER, not the width,
// so assert the order instead of the type.
// Scope to the struct body: the module docstring deliberately quotes the old
// wrong order, so a whole-file scan would read the documentation as the code.
const structBody = (() => {
  const start = inspector.indexOf("class AVFilter(C.Structure)");
  if (start < 0) return "";
  const end = inspector.indexOf("\n]", start);
  return end < 0 ? inspector.slice(start) : inspector.slice(start, end);
})();
const fieldOrder = [...structBody.matchAll(/\("([a-z_0-9]+)",\s*C\./g)].map((m) => m[1]);
const expectedOrder = ["name", "description", "inputs", "outputs", "priv_class", "flags",
                       "nb_inputs", "nb_outputs", "process_command"];
if (!structBody || expectedOrder.some((field) => !fieldOrder.includes(field))) {
  failures.push(
    `inspect-filter-runtime.py does not mirror the FFmpeg 6.0 AVFilter field order `
      + `(found: ${fieldOrder.slice(0, 12).join(", ")}). The wrong order produced a false `
      + `"extrastereo has no process_command" reading.`,
  );
}
const indexOf = (field) => fieldOrder.indexOf(field);
if (indexOf("inputs") > indexOf("priv_class") || indexOf("outputs") > indexOf("priv_class")) {
  failures.push(
    "inspect-filter-runtime.py places inputs/outputs after priv_class; FFmpeg 6.0 "
      + "declares them before it, which shifts every later field.",
  );
}
if (!/nb_inputs",\s*C\.c_uint8/.test(structBody)) {
  failures.push(
    "inspect-filter-runtime.py does not read the pad counts as uint8; FFmpeg 6.0 "
      + "packs nb_inputs/nb_outputs/formats_state as bytes, so a 32-bit read shifts "
      + "every later field.",
  );
}
if (!/avfilter_filter_pad_count/.test(inspector)) {
  failures.push(
    "inspect-filter-runtime.py does not cross-check the layout against "
      + "avfilter_filter_pad_count, so a wrong layout would again be reported as "
      + "a per-filter capability.",
  );
}
if (!/MAX_PLAUSIBLE_PRIV_SIZE/.test(inspector)) {
  failures.push(
    "inspect-filter-runtime.py has no plausibility bound on priv_size, so a "
      + "misaligned read would not be caught.",
  );
}

for (const note of notes) console.log(`  note: ${note}`);
if (failures.length) {
  console.error(`\ntask AFX-1: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("task AFX-1: audio-effects evidence discipline verified");
