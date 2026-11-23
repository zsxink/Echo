import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./ci-perf-paths.mjs", import.meta.url));
const css = "apps/desktop/src/styles/library.css";
const otherCss = "apps/desktop/src/styles/player.css";
const docs = "docs/DESIGN.md";
const rust = "crates/echo-core/src/lib.rs";
const readme = "README.md";
const prototype = "docs/prototype/echo-desktop-player.html";
const initial = [css, docs, rust, readme, prototype];

function classify(t, setup, values = {}) {
  const root = mkdtempSync(join(tmpdir(), "echo-perf-diff-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  const write = (path, content = "changed\n") => {
    const full = join(root, path);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, content);
  };
  const commit = (message) => { git("add", "-A"); git("commit", "-qm", message); return git("rev-parse", "HEAD"); };
  git("init", "-q");
  git("config", "user.name", "Fixture");
  git("config", "user.email", "fixture@example.invalid");
  for (const path of initial) write(path, "original\n");
  const base = commit("base");
  const endpoints = setup({ git, write, commit, base, root }) ?? {};
  const output = join(root, "output");
  execFileSync(process.execPath, [script], {
    cwd: root,
    env: {
      ...process.env,
      GITHUB_EVENT_NAME: "pull_request",
      ECHO_PR_BASE_SHA: base,
      ECHO_PR_HEAD_SHA: git("rev-parse", "HEAD"),
      GITHUB_OUTPUT: output,
      ...endpoints,
      ...values,
    },
  });
  return readFileSync(output, "utf8");
}

test("audited CSS and docs modifications may skip", (t) => {
  assert.equal(classify(t, ({ write, commit }) => { write(css); write(docs); commit("audited"); }), "skip=true\n");
});

test("audited visual prototype and generated CSS may skip together", (t) => {
  assert.equal(classify(t, ({ write, commit }) => { write(prototype); write(css); commit("prototype"); }), "skip=true\n");
});

test("PR comparison ignores changes made only on advanced base", (t) => {
  assert.equal(classify(t, ({ git, write, commit }) => {
    git("checkout", "-qb", "base-advanced");
    write(rust);
    const nextBase = commit("base only");
    git("checkout", "-qb", "pr", "HEAD^");
    write(css);
    commit("pr only");
    return { ECHO_PR_BASE_SHA: nextBase };
  }), "skip=true\n");
});

for (const [label, change] of [
  ["mixed CSS and Rust", ({ write }) => { write(css); write(rust); }],
  ["README outside whitelist", ({ write }) => write(readme)],
  ["new file even on allowlist", ({ write }) => write(otherCss)],
  ["deleted allowlisted file", ({ git }) => git("rm", css)],
  ["rename between two allowlisted paths", ({ git }) => git("mv", css, otherCss)],
  ["unknown path with newline", ({ write }) => write("docs/unknown\nfile.md")],
  ["file mode change", ({ root }) => chmodSync(join(root, css), 0o755)],
]) {
  test(`${label} runs performance`, (t) => {
    assert.equal(classify(t, ({ write, commit, git, root }) => { change({ write, git, root }); commit(label); }), "skip=false\n");
  });
}

test("empty diff, missing or malformed endpoints run performance", (t) => {
  assert.equal(classify(t, () => {}), "skip=false\n");
  assert.equal(classify(t, () => {}, { ECHO_PR_HEAD_SHA: "1".repeat(40) }), "skip=false\n");
  assert.equal(classify(t, () => {}, { ECHO_PR_BASE_SHA: "" }), "skip=false\n");
  assert.equal(classify(t, () => {}, { ECHO_PR_BASE_SHA: "HEAD" }), "skip=false\n");
});

test("non-PR workflow events never skip", (t) => {
  assert.equal(classify(t, ({ write, commit }) => { write(css); commit("audited"); }, { GITHUB_EVENT_NAME: "push" }), "skip=false\n");
});
