#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { RELEASE_LATENCY_REPORT } from "./release-edit-latency.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const reports = [];
const limitMs = 50;
const samples = 100;
const operations = ["favorite", "create_playlist", "add_to_playlist", "remove_from_playlist"];
const marker = "ECHO_LOCAL_EDIT_PERF ";
const reportFile = resolve(root, RELEASE_LATENCY_REPORT);

function distribution(metric) {
  return metric?.samples === samples && ["min_ms", "p50_ms", "p95_ms", "p99_ms", "max_ms", "mean_ms"]
    .every((key) => Number.isFinite(metric[key]) && metric[key] >= 0);
}

for (const songs of [1_000, 10_000]) {
  // The Release acceptance entry owns its dimensions. Diagnostic bench modes,
  // caller fixture paths, and relaxed thresholds never leak into this run.
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("ECHO_PERF_") && key !== "ECHO_EDIT_TRACE"));
  Object.assign(env, { ECHO_PERF_SONGS: String(songs), ECHO_PERF_SAMPLES: String(samples), ECHO_PERF_MAX_P95_MS: String(limitMs) });
  const measured = spawnSync("cargo", [
    "test", "--locked", "-p", "echo-desktop", "--all-features", "--release",
    "--test", "local_edit_performance", "completed_local_edit_latency", "--",
    "--exact", "--ignored", "--nocapture", "--test-threads=1",
  ], { cwd: root, env, encoding: "utf8", maxBuffer: 10 * 1024 * 1024 });
  if (measured.stdout) process.stdout.write(measured.stdout);
  if (measured.stderr) process.stderr.write(measured.stderr);
  const errors = [];
  if (measured.status !== 0) errors.push(`cargo failed: ${measured.error ?? measured.status ?? measured.signal}`);
  let result = null;
  const rows = measured.stdout?.split("\n").filter((line) => {
    const at = line.indexOf(marker);
    return at >= 0 && line[at + marker.length] === "{";
  }) ?? [];
  if (rows.length !== 1) errors.push(`expected one benchmark JSON row, found ${rows.length}`);
  else {
    try { result = JSON.parse(rows[0].slice(rows[0].indexOf(marker) + marker.length)); }
    catch { errors.push("invalid benchmark JSON"); }
  }
  if (result?.songs !== songs || result?.build !== "release") errors.push("wrong benchmark scale/profile");
  for (const name of operations) {
    const op = result?.[name];
    // #152 makes a successful favorite commit project inline; do not invent a
    // background phase. Playlist operations retain their real background path.
    const correctProjection = name === "favorite"
      ? op?.projection_mode === "foreground" && distribution(op.foreground_projection) && op.background_projection === null
      : op?.projection_mode === "background" && distribution(op.background_projection);
    if (!distribution(op) || !distribution(op?.directory_commit) || !correctProjection) {
      errors.push(`${name} missing complete command/commit/projection evidence`);
    } else if (!(op.p95_ms < limitMs)) {
      errors.push(`${name} p95 ${op.p95_ms} ms is not below ${limitMs} ms`);
    }
  }
  reports.push({ songs, result, errors });
  if (errors.length) process.stderr.write(`FAIL release edit latency (${songs} songs): ${errors.join("; ")}\n`);
}
const passed = reports.every(({ errors }) => errors.length === 0);
mkdirSync(dirname(reportFile), { recursive: true });
writeFileSync(reportFile, JSON.stringify({ passed, limit_ms: limitMs, samples_per_operation: samples, reports }, null, 2) + "\n");
if (!passed) process.exitCode = 1;
else process.stdout.write("ok release edit latency: 1,000 + 10,000 songs; command p95 < 50 ms; commit and applicable projection recorded\n");
