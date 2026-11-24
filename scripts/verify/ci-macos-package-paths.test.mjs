import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./ci-macos-package-paths.mjs", import.meta.url));
const doc = "docs/superpowers/plans/2026-11-17-ci-runtime-under-10-min.md";
const testOnly = "scripts/verify/checks/task-1.7.test.mjs";
const css = "apps/desktop/src/styles/library.css";
const tauri = "apps/desktop/src-tauri/tauri.conf.json";
const rust = "crates/echo-core/src/lib.rs";
const lock = "pnpm-lock.yaml";
const gate = "scripts/verify/checks/task-1.9.mjs";

function classify(t, change, extras = {}) {
  const root = mkdtempSync(join(tmpdir(), "echo-macos-package-diff-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  const write = (path, text = "changed\n") => {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  };
  const commit = () => { git("add", "-A"); git("commit", "-qm", "fixture"); return git("rev-parse", "HEAD"); };
  git("init", "-q");
  git("config", "user.name", "Fixture");
  git("config", "user.email", "fixture@example.invalid");
  for (const path of [doc, testOnly, css, rust, tauri, lock, gate]) write(path, "original\n");
  const base = commit();
  const endpoints = change({ git, write, commit, root }) || {};
  const output = join(root, "output");
  execFileSync(process.execPath, [script], {
    cwd: root,
    env: {
      ...process.env,
      GITHUB_EVENT_NAME: "pull_request",
      ECHO_PR_BASE_SHA: base,
      ECHO_PR_HEAD_SHA: git("rev-parse", "HEAD"),
      GITHUB_OUTPUT: output,
      ...endpoints, ...extras,
    },
  });
  return readFileSync(output, "utf8");
}

test("historical markdown or an existing Linux-only verification test may skip", (t) => {
  assert.equal(classify(t, ({ write, commit }) => { write(doc); write(testOnly); commit(); }), "skip=true\n");
});
test("main and invalid or empty diffs fail open to package", (t) => {
  const safe = ({ write, commit }) => { write(doc); commit(); };
  assert.equal(classify(t, safe, { GITHUB_EVENT_NAME: "push" }), "skip=false\n");
  assert.equal(classify(t, safe, { ECHO_PR_BASE_SHA: "0".repeat(40) }), "skip=false\n");
  assert.equal(classify(t, safe, { ECHO_PR_HEAD_SHA: "HEAD" }), "skip=false\n");
  assert.equal(classify(t, () => {}), "skip=false\n");
});
test("compares complete three-point PR diff, not advanced main changes", (t) => {
  assert.equal(classify(t, ({ git, write, commit }) => {
    git("checkout", "-qb", "advanced");
    write(rust);
    const advanced = commit();
    git("checkout", "-qb", "pr", "HEAD^");
    write(doc);
    commit();
    return { ECHO_PR_BASE_SHA: advanced };
  }), "skip=true\n");
});
for (const [name, edit] of [
  ["CSS", ({ write }) => write(css)],
  ["Rust source", ({ write }) => write(rust)],
  ["Tauri configuration", ({ write }) => write(tauri)],
  ["lockfile", ({ write }) => write(lock)],
  ["packaging check", ({ write }) => write(gate)],
  ["mixed paths", ({ write }) => { write(doc); write(css); }],
  ["unknown document", ({ write }) => write("docs/unknown\nstring.md")],
  ["new allowlisted path", ({ write }) => write("docs/superpowers/plans/2026-11-20-new.md")],
  ["delete a safe path", ({ git }) => git("rm", doc)],
  ["rename between safe paths", ({ git }) => git("mv", doc, "docs/superpowers/plans/2026-11-20-renamed.md")],
  ["file mode change", ({ root }) => chmodSync(join(root, doc), 0o755)],
]) {
  test(`${name} runs full packaging`, (t) => {
    assert.equal(classify(t, (fixture) => { edit(fixture); fixture.commit(); }), "skip=false\n");
  });
}
