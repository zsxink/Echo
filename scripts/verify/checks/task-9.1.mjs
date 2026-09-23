#!/usr/bin/env node
// Task 9.1 check: productized single-instance + initialization request FIFO.
//
// The shell wires both OS file-open paths (single-instance argv and
// RunEvent::Opened) through the StartupSupervisor so that file-opens
// received while the runtime is initializing are stashed and drained once
// ready — never silently dropped, and handled only by the already-running
// main instance.
//
// Fails (nonzero) unless:
//   1. the `echo-app` binary compiles and is clippy-clean on this host;
//   2. the echo-desktop runtime tests pass (bounded, keeps-newest,
//      drains-in-order, ready-drain, frontend-ready gate and later-opens-
//      immediate);
//   3. the shell actually routes both open paths through the supervisor
//      (grep-level structural guard against a regression to a direct,
//      un-FIFO'd `app.emit`);
//   4. the macOS `RunEvent::Opened` payload is normalized to filesystem paths
//      via `open_targets` (normalize-os-file-open-paths): the URL→path
//      conversion unit tests pass on this host, and no `url.to_string()`
//      survives in the shell;
//   5. the frontend-ready delivery gate exists (deliver-file-opens-after-
//      frontend-ready): the shell's drain goes through `mark_frontend_ready`
//      (never `on_ready`, which would emit into a WebView with no JS listener
//      and be silently dropped by Tauri), the ready command is registered, and
//      the frontend signals readiness from the same effect that subscribes to
//      the file-open event.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.1: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

// 1. The thin binary stays thin but now wires the runtime FIFO: it must
//    compile and be clippy-clean.
run("cargo", ["check", "-p", "echo-app", "--locked"], "echo-app compiles");
run(
  "cargo",
  ["clippy", "-p", "echo-app", "--", "-D", "warnings"],
  "echo-app clippy clean",
);

// 2. The FIFO semantics backing task 9.1 are proven by the runtime tests.
const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "runtime::"],
  "echo-desktop runtime tests",
);
for (const name of [
  "supervisor_sequence_resolves_gate_and_drains_pending_opens",
  "pending_open_fifo_is_bounded_and_keeps_newest",
  "frontend_ready_gate_is_idempotent_and_never_replays",
  "gate_kind_maps_recovery_states",
]) {
  if (!testOut.includes(`test runtime::tests::${name} ... ok`)) {
    fail(`runtime test not green: ${name}`);
  }
}
// The composition-root single-supervisor assertion lives in the services
// module (runtime::services::tests::open_path), which already existed before
// this change.
if (
  !testOut.includes(
    "test runtime::services::tests::open_path::the_composition_root_shares_the_shells_startup_supervisor ... ok",
  )
) {
  fail("runtime test not green: the_composition_root_shares_the_shells_startup_supervisor");
}

// 3. Structural guard: the shell routes both OS open paths through the
//    supervisor's FIFO instead of emitting directly.
const main = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
const required = [
  "StartupSupervisor",
  "receive_file_open",
  "on_ready",
  "deliver_file_open",
  "drain_pending_opens",
  "app://file-open-request",
];
for (const token of required) {
  if (!main.includes(token)) {
    fail(`main.rs no longer routes through '${token}'`);
  }
}
// A regression where the single-instance callback bypasses the FIFO and emits
// directly would leave an unguarded `app.emit(FILE_OPEN_REQUEST, ...)` on the
// argv path. Both open paths must go through `deliver_file_open`, so the only
// direct emits to that event live in `deliver_file_open`/`drain_pending_opens`.
const directEmits = (main.match(/app\.emit\(FILE_OPEN_REQUEST/g) || []).length;
if (directEmits > 2) {
  fail(`unexpected direct file-open emits in shell: ${directEmits}`);
}

// 4. Behavior guard for the OS-boundary normalization
//    (normalize-os-file-open-paths). The structural tokens alone cannot tell a
//    decoded path from a raw `file://` string (the original bug slipped past
//    them), so the behavior itself is pinned by the `open_targets` unit tests,
//    which run on every host even though the only caller is macOS-only.
const appTestOut = run(
  "cargo",
  ["test", "-p", "echo-app", "--locked", "open_targets"],
  "echo-app open_targets tests",
);
for (const name of [
  "decodes_percent_encoding_and_keeps_order",
  "drops_non_file_schemes",
  "mixed_input_keeps_only_file_targets_in_order",
]) {
  if (!appTestOut.includes(`test open_targets::tests::${name} ... ok`)) {
    fail(`open_targets test not green: ${name}`);
  }
}
if (main.includes("url.to_string()")) {
  fail(
    "main.rs stringifies a URL again (url.to_string()): the Opened payload must be normalized through open_targets, never handed downstream as a URL string",
  );
}
if (!main.includes("open_targets::open_targets(&urls)")) {
  fail("the RunEvent::Opened branch no longer normalizes payloads via open_targets");
}

// 5. Frontend-ready delivery gate (deliver-file-opens-after-frontend-ready).
//    The FIFO by itself does not fix the cold-start bug: a path drained while
//    the WebView is still booting is emitted into a window with no registered
//    JS listener, and Tauri silently drops it. The shell therefore delivers
//    only once the frontend reports that its file-open listener is in place.
function functionBody(src, fnName) {
  const sigIdx = src.indexOf(`fn ${fnName}(`);
  if (sigIdx === -1) return "";
  const open = src.indexOf("{", sigIdx);
  let depth = 0;
  let i = open;
  for (; i < src.length; i += 1) {
    if (src[i] === "{") depth += 1;
    else if (src[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        i += 1;
        break;
      }
    }
  }
  return src.slice(open, i);
}
if (!main.includes("mark_frontend_ready")) {
  fail("main.rs no longer references the frontend-ready gate (mark_frontend_ready)");
}
if (!main.includes("file_open_frontend_ready")) {
  fail("main.rs no longer registers the frontend-ready command (file_open_frontend_ready)");
}
// The drain must go through `mark_frontend_ready`, never `on_ready`: `on_ready`
// runs at shell-setup end when the WebView has not loaded, so draining there
// would emit into silence — resurrecting the exact cold-start drop this change
// exists to prove against.
const drainBody = functionBody(main, "drain_pending_opens");
if (!drainBody.includes("mark_frontend_ready")) {
  fail("drain_pending_opens no longer drains via mark_frontend_ready");
}
if (drainBody.includes("on_ready")) {
  fail("drain_pending_opens drains via on_ready again: a cold-start emit into an unloaded WebView is dropped by Tauri");
}
// The frontend detonates the gate from the effect that registers the listener:
// no listener, no flush — the queued paths wait, they are not lost.
const appTsx = readFileSync(resolve(ROOT, "apps", "desktop", "src", "app", "App.tsx"), "utf8");
if (!appTsx.includes("file_open_frontend_ready")) {
  fail("App.tsx no longer signals frontend readiness after subscribing to app://file-open-request");
}

// 6. Process-wide supervisor created before the Tauri application exists
//    (deliver-file-opens-after-frontend-ready, D6). On a real machine macOS
//    delivers the cold-start `odoc` BEFORE `.setup()` runs, so a supervisor
//    registered only inside `.setup()` is still absent exactly when the first
//    and only open request of that launch arrives: `try_state` returns `None`
//    and the path is dropped — the app starts and never plays. Measured order
//    on macOS: RunEvent::Opened -> deliver_file_open -> "setup".
//    The gate above (5) is unreachable while that holds, so this is the
//    precondition the whole change rests on and it must be pinned structurally.
const createdIdx = main.indexOf("Arc::new(StartupSupervisor::new())");
if (createdIdx === -1) {
  fail("main.rs no longer creates the process-wide StartupSupervisor");
}
const builderIdx = main.indexOf("tauri::Builder::default()");
if (builderIdx === -1) {
  fail("main.rs no longer builds the Tauri application");
}
if (createdIdx > builderIdx) {
  fail(
    "the StartupSupervisor is created after `tauri::Builder::default()`: a cold-start odoc arrives before .setup(), so the path has nowhere to queue and a double-click only launches the app",
  );
}
// The pre-fix shape looked the supervisor up through managed state, which is
// `None` at cold-start odoc time — exactly the drop. No shell code may fall
// back to that lookup for open delivery again.
if (/try_state::<Arc<StartupSupervisor>>/.test(main)) {
  fail(
    "main.rs looks the supervisor up via `try_state::<Arc<StartupSupervisor>>` again (D6): at cold-start odoc time the state is not registered yet, so the path is dropped and the double-click only launches the app",
  );
}
// Both OS open paths must *pass* the instance created before the Builder, so a
// solution that keeps the captured-arg signature but hands it a fresh
// supervisor still fails here.
const capturedCalls = (main.match(/deliver_file_open\(app, &startup/g) || []).length;
if (capturedCalls < 2) {
  fail(
    `only ${capturedCalls} open path(s) pass the captured supervisor into deliver_file_open (expected the RunEvent::Opened and single-instance paths)`,
  );
}

process.stdout.write(
  "ok 9.1: process-wide supervisor before Builder, single-instance + init FIFO wiring, frontend-ready delivery gate, open normalization, runtime FIFO tests green\n",
);
