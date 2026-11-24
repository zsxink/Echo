import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

for (const mode of ["pass", "threshold", "missing_phase", "cargo_failure"]) {
  test(`the Release latency entry reports both scales: ${mode}`, () => {
    const root = mkdtempSync(join(tmpdir(), "echo-edit-latency-"));
    try {
      const verify = join(root, "scripts", "verify");
      mkdirSync(verify, { recursive: true });
      for (const file of ["run-release-edit-latency.mjs", "release-edit-latency.mjs"]) {
        copyFileSync(fileURLToPath(new URL(file, import.meta.url)), join(verify, file));
      }
      const bin = join(root, "bin");
      mkdirSync(bin);
      const cargo = join(bin, "cargo");
      writeFileSync(cargo, `#!/usr/bin/env node
const assert = require("node:assert/strict");
const e = process.env;
const songs = Number(e.ECHO_PERF_SONGS);
assert.ok([1000, 10000].includes(songs));
assert.equal(e.ECHO_PERF_MAX_P95_MS, "50");
assert.equal(e.ECHO_PERF_SAMPLES, "100");
assert.equal(e.ECHO_PERF_SERVE, undefined);
assert.equal(e.ECHO_PERF_FIXTURE_PATH, undefined);
assert.equal(e.ECHO_EDIT_TRACE, undefined);
for (const flag of ["--release", "--locked", "--exact", "--ignored", "completed_local_edit_latency"]) assert.ok(process.argv.includes(flag));
const metric = { samples: 100, min_ms: 1, p50_ms: 2, p95_ms: 3, p99_ms: 4, max_ms: 5, mean_ms: 3 };
const result = { songs, build: "release" };
for (const name of ["favorite", "create_playlist", "add_to_playlist", "remove_from_playlist"]) {
  const base = { ...metric, directory_commit: metric };
  result[name] = name === "favorite"
    ? { ...base, projection_mode: "foreground", foreground_projection: metric, background_projection: null }
    : { ...base, projection_mode: "background", background_projection: metric };
}
const mode = ${JSON.stringify(mode)};
if (songs === 1000 && mode === "threshold") result.favorite.p95_ms = 50;
if (songs === 1000 && mode === "missing_phase") delete result.add_to_playlist.background_projection;
if (songs === 1000 && mode === "cargo_failure") process.exit(101);
process.stdout.write("test completed_local_edit_latency ... ECHO_LOCAL_EDIT_PERF " + JSON.stringify(result) + "\\n");
`, { mode: 0o755 });
      const result = spawnSync(process.execPath, [join(verify, "run-release-edit-latency.mjs")], {
        encoding: "utf8",
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, ECHO_PERF_SONGS: "10",
          ECHO_PERF_SAMPLES: "2", ECHO_PERF_MAX_P95_MS: "500", ECHO_PERF_SERVE: "1", ECHO_EDIT_TRACE: "1",
          ECHO_PERF_FIXTURE_PATH: "/changed/fixture" },
      });
      const report = JSON.parse(readFileSync(join(root, "artifacts", "release-edit-latency.json"), "utf8"));
      assert.equal(result.status, mode === "pass" ? 0 : 1, result.stderr);
      assert.equal(report.passed, mode === "pass");
      assert.deepEqual(report.reports.map((entry) => entry.songs), [1000, 10000]);
      assert.deepEqual(report.reports[1].errors, []);
      const expected = { pass: null, threshold: /not below 50/, missing_phase: /missing complete/, cargo_failure: /cargo failed/ }[mode];
      if (expected) assert.match(report.reports[0].errors.join(" "), expected);
      else assert.deepEqual(report.reports[0].errors, []);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
