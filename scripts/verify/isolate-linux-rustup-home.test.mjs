import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const helper = fileURLToPath(new URL("./isolate-linux-rustup-home.sh", import.meta.url));
const linux = { skip: process.platform !== "linux" };

test("Linux Rustup setup uses one stable home and does not change Cargo configuration", linux, (t) => {
  const temp = mkdtempSync(join(tmpdir(), "echo-rustup-env-"));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  const envFile = join(temp, "github-env");
  const home = join(temp, "runner home");
  const expected = `CARGO_HOME=/preexisting/.cargo\nRUSTUP_HOME=${home}/.rustup-echo-ci\n`;
  for (const previous of ["/runner/.rustup", "/image-upgrade/.rustup"]) {
    writeFileSync(envFile, "CARGO_HOME=/preexisting/.cargo\n");
    const r = spawnSync("bash", [helper], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, RUSTUP_HOME: previous, GITHUB_ENV: envFile },
    });
    assert.ifError(r.error);
    assert.equal(r.status, 0, r.stderr);
    assert.equal(readFileSync(envFile, "utf8"), expected);
  }
});

test("Linux Rustup setup fails if the runner did not provide an environment file", linux, () => {
  const r = spawnSync("bash", [helper], {
    encoding: "utf8",
    env: { ...process.env, GITHUB_ENV: "" },
  });
  assert.ifError(r.error);
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /GITHUB_ENV/);
});
