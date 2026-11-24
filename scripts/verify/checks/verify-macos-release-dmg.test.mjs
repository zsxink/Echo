import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { exactDmg, verifyMacosReleaseDmg } from "./verify-macos-release-dmg.mjs";

function fixture(t, mode) {
  const root = mkdtempSync(join(tmpdir(), "echo-release-dmg-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const dmg = join(root, "Echo.dmg");
  const distIndex = join(root, "index.html");
  const calls = [];
  writeFileSync(dmg, "exact bytes");
  writeFileSync(distIndex, "<script src='/assets/current.js'></script>");
  async function run() {
    return verifyMacosReleaseDmg({
      dmgDirectory: root, distIndex, macOS: true,
      command(command, args, options) {
        calls.push({ command, args, options });
        if (command === "hdiutil" && args[0] === "attach") {
          const mount = args[args.indexOf("-mountpoint") + 1];
          assert.equal(args.at(-1), exactDmg(root));
          assert.ok(args.includes("-readonly"));
          if (mode === "no-app") return;
          if (mode === "symlink-app") {
            symlinkSync(root, join(mount, "Echo.app"), "dir");
            return;
          }
          mkdirSync(join(mount, "Echo.app/Contents"), { recursive: true });
        }
        if (command === "codesign" && mode === "invalid-signature") throw new Error("invalid signature");
        if (command === "pnpm" && mode === "native-gate-failed") throw new Error("9.7 failed");
      },
      inspect(app, { distIndex: matchedDist }) {
        calls.push({ command: "inspect", args: [app, matchedDist] });
        assert.equal(matchedDist, distIndex);
        if (mode === "stale") throw new Error("strict embedded resources mismatch");
        if (mode === "mutated") writeFileSync(dmg, "tampered product");
      },
    });
  }
  return { root, dmg, calls, run };
}

test("validates one readonly mounted DMG app, not a staged app; no build or signing calls", async (t) => {
  const { calls, run } = fixture(t);
  await run();
  assert.deepEqual(calls.map((call) => call.command), ["hdiutil", "codesign", "inspect", "pnpm", "hdiutil"]);
  const mount = calls[0].args.at(-2);
  const app = join(mount, "Echo.app");
  assert.equal(calls[1].args[0], "--verify");
  assert.equal(calls[1].args.at(-1), app);
  assert.equal(calls[2].args[0], app);
  assert.equal(calls[3].options.env.ECHO_MACOS_BUNDLE, join(app, "Contents"));
  assert.deepEqual(calls[3].args, ["verify:task", "--", "1.10", "9.7"]);
  assert.deepEqual(calls[4].args, ["detach", mount]);
});
for (const [mode, expected] of [
  ["no-app", /Echo\.app/], ["symlink-app", /no symlink/], ["invalid-signature", /invalid signature/],
  ["stale", /strict embedded/], ["native-gate-failed", /9\.7 failed/], ["mutated", /bytes changed/],
]) {
  test(`${mode} fails and still detaches the exact mounted volume`, async (t) => {
    const { calls, run } = fixture(t, mode);
    await assert.rejects(run, expected);
    assert.equal(calls.at(-1).args[0], "detach");
    if (["no-app", "symlink-app", "invalid-signature", "stale"].includes(mode)) {
      assert.ok(!calls.some((call) => call.command === "pnpm"));
    }
  });
}
test("missing, ambiguous or symlinked DMG cannot be selected", (t) => {
  const { dmg, root } = fixture(t);
  writeFileSync(join(root, "Other.dmg"), "different");
  assert.throws(() => exactDmg(root), /exactly one/);
  rmSync(join(root, "Other.dmg"));
  rmSync(dmg);
  assert.throws(() => exactDmg(root), /exactly one/);
  symlinkSync(join(root, "index.html"), dmg);
  assert.throws(() => exactDmg(root), /regular/);
});
