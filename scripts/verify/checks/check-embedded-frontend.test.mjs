import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

test("strict embeds every current asset and fails closed for dev or missing explicit binary", (t) => {
  const root = mkdtempSync(join(tmpdir(), "echo-strict-embedded-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const script = fileURLToPath(new URL("./check-embedded-frontend.mjs", import.meta.url));
  const binary = join(root, "Echo");
  const index = join(root, "index.html");
  writeFileSync(index, '<script src="/assets/current.js"></script><link href="/assets/current.css">');
  const run = (...args) => spawnSync(process.execPath, [script, "--dist-index", index, ...args], { encoding: "utf8" });
  assert.equal(run("--strict").status, 1);
  assert.equal(run("--strict", "--bin", binary).status, 1);

  writeFileSync(binary, "http://127.0.0.1:1420");
  assert.match(run("--bin", binary).stdout, /^SKIP embedded-frontend/);
  const dev = run("--strict", "--bin", binary);
  assert.equal(dev.status, 1);
  assert.match(dev.stderr, /do not accept dev-server/);

  writeFileSync(binary, "old.js old.css");
  assert.equal(run("--strict", "--bin", binary).status, 1);
  writeFileSync(binary, "current.js old.css");
  assert.equal(run("--strict", "--bin", binary).status, 1);
  writeFileSync(binary, "current.js current.css");
  const good = run("--strict", "--bin", binary);
  assert.equal(good.status, 0);
  assert.match(good.stdout, /^ok embedded-frontend: 2 asset/);
});
