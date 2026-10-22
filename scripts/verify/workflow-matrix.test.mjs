import { test } from "node:test";
import assert from "node:assert/strict";
import { assertPlatformMatrix, matrixEntries, parseWorkflow } from "./workflow-matrix.mjs";

const expected = { Linux: { os: "ubuntu-latest" }, macOS: { os: "xcode-27" }, Windows: { os: "windows-latest" } };
const entries = [
  { platform: "Linux", os: "ubuntu-latest" },
  { platform: "macOS", os: "xcode-27" },
  { platform: "Windows", os: "windows-latest" },
];

test("valid platform mappings are accepted independent of order", () => {
  assert.doesNotThrow(() => assertPlatformMatrix([...entries].reverse(), expected, "CI"));
});
test("missing platform is rejected", () => {
  assert.throws(() => assertPlatformMatrix(entries.slice(1), expected, "CI"), /missing platform 'Linux'/);
});
test("runner mapped to the wrong platform is rejected", () => {
  assert.throws(() => assertPlatformMatrix([{ platform: "Linux", os: "windows-latest" }, ...entries.slice(1)], expected, "CI"), /Linux.*expected 'ubuntu-latest'/);
});
test("duplicate platforms cannot silently replace an entry", () => {
  assert.throws(() => assertPlatformMatrix([...entries, entries[0]], expected, "CI"), /duplicate/);
});
test("malformed YAML and duplicate YAML keys are rejected", () => {
  for (const source of ["jobs: [", "jobs: {}\njobs: {}", "[]"]) {
    assert.throws(() => parseWorkflow(source, "CI"), /not valid YAML/);
  }
});
test("extra matrix axes and runner selection drift are rejected", () => {
  for (const job of [
    { "runs-on": "ubuntu-latest", strategy: { matrix: { include: entries } } },
    { "runs-on": "${{ matrix.os }}", strategy: { matrix: { include: entries, os: ["ubuntu-latest"] } } },
  ]) {
    assert.throws(() => matrixEntries({ jobs: { rust: job } }, "rust", "CI"));
  }
});
