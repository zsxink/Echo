// Check the exact final DMG selected by the release upload; never rebuild or
// re-sign its contents. The only app accepted is its read-only mounted App.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, lstatSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { inspectMacApp } from "./inspect-macos-app.mjs";

const ROOT = fileURLToPath(new URL("../../../", import.meta.url));

export function exactDmg(directory) {
  const matches = readdirSync(directory, { withFileTypes: true }).filter((file) => file.name.endsWith(".dmg"));
  if (matches.length !== 1 || !matches[0].isFile()) {
    throw new Error(`expected exactly one packaged, regular .dmg under ${directory}; found ${matches.length}`);
  }
  return join(realpathSync(directory), matches[0].name);
}

async function sha256(path) {
  const hash = createHash("sha256");
  for await (const part of createReadStream(path)) hash.update(part);
  return hash.digest("hex");
}

function run(command, args, { env, timeout = 120000 } = {}) {
  execFileSync(command, args, { cwd: ROOT, env, timeout, stdio: "inherit" });
}

export async function verifyMacosReleaseDmg({
  dmgDirectory, distIndex, reportPath, command = run, inspect = inspectMacApp, macOS = process.platform === "darwin",
}) {
  if (!macOS) throw new Error("release DMG verification requires macOS");
  const dmg = exactDmg(dmgDirectory);
  const dist = realpathSync(distIndex);
  if (!lstatSync(dist).isFile()) throw new Error("release dist must be a regular index HTML file");
  const before = await sha256(dmg);
  const scratch = realpathSync(mkdtempSync(join(tmpdir(), "echo-macos-release-")));
  const mount = join(scratch, "mount");
  mkdirSync(mount);
  const originalDevice = statSync(mount).dev;
  let attached = false;
  try {
    command("hdiutil", ["attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", mount, dmg]);
    attached = true;
    const app = join(mount, "Echo.app");
    if (!lstatSync(app).isDirectory() || realpathSync(app) !== app) {
      throw new Error("the DMG must contain its own Echo.app directory (no symlink or staged fallback)");
    }
    command("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);
    await inspect(app, { distIndex: dist });
    command("pnpm", ["verify:task", "--", "1.10", "9.7"], {
      env: {
        ...process.env,
        ECHO_MACOS_BUNDLE: join(app, "Contents"),
        ECHO_MACOS_REPORT: reportPath ? resolve(reportPath) : join(scratch, "echo-libmpv-report.md"),
      },
    });
  } finally {
    // A failed attach may still mount a volume before the client errors.
    // Never delete a mounted tree; require detach before scratch cleanup.
    if (attached || statSync(mount).dev !== originalDevice) command("hdiutil", ["detach", mount]);
    rmSync(scratch, { recursive: true, force: true });
  }
  if (before !== await sha256(dmg)) throw new Error("release DMG bytes changed during verification");
  console.log(`ok macOS release DMG: checked original signed app, SHA-256 ${before}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [dmgDirectory, distIndex, reportPath, ...extra] = process.argv.slice(2);
  try {
    if (!dmgDirectory || !distIndex || !reportPath || extra.length) {
      throw new Error("usage: verify-macos-release-dmg.mjs <dmg-directory> <this-build-dist-index.html> <libmpv-report-path>");
    }
    await verifyMacosReleaseDmg({ dmgDirectory, distIndex, reportPath });
  } catch (error) {
    console.error(`FAIL macOS release DMG: ${error.message}`);
    process.exitCode = 1;
  }
}
