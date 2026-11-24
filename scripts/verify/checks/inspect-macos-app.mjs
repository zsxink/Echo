// Read-only inspection of one explicit already-built Echo.app, shared by the
// temporary CI package and the signed App mounted from the release DMG.
import { execFileSync, spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { isPackagedDylibReference } from "../../release/thin-macos-libmpv.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const FIXTURE = resolve(ROOT, "fixtures", "audio", "tone-short.mp3");
const MANIFEST = resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "macos", "manifest.json");
const REQUIRED_EXTENSIONS = ["mp3", "flac", "m4a", "ogg", "opus", "wav", "ape"];
const sleep = (milliseconds) => new Promise((done) => setTimeout(done, milliseconds));

function readonlyCommand(command, args) {
  return execFileSync(command, args, { encoding: "utf8", timeout: 30000 });
}

function requireThat(ok, reason) {
  if (!ok) throw new Error(reason);
}

function validateIdentity(bundle, run) {
  const contents = join(realpathSync(bundle), "Contents");
  const info = join(contents, "Info.plist");
  const macos = join(contents, "MacOS");
  const executableName = run("plutil", ["-extract", "CFBundleExecutable", "raw", "-o", "-", info]).trim();
  requireThat(/^[A-Za-z0-9_.-]+$/.test(executableName) && ![".", ".."].includes(executableName),
    "invalid app executable name in Info.plist");
  const configuredExecutable = join(macos, executableName);
  const executable = realpathSync(configuredExecutable);
  requireThat(dirname(executable) === macos && executable === configuredExecutable,
    "bundle executable must be a regular path inside Contents/MacOS");
  const documentTypes = JSON.parse(run("plutil", ["-extract", "CFBundleDocumentTypes", "json", "-o", "-", info]));
  const extensions = new Set(documentTypes.flatMap((entry) => entry.CFBundleTypeExtensions ?? []));
  for (const ext of REQUIRED_EXTENSIONS) requireThat(extensions.has(ext), `Info.plist is missing the .${ext} file association`);

  const frameworks = join(contents, "Frameworks");
  const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
  for (const target of [executable, ...Object.keys(manifest.files).map((path) => join(frameworks, path))]) {
    requireThat(existsSync(target), `missing bundle executable or framework: ${target}`);
    const architectures = run("lipo", ["-archs", target]).trim().split(/\s+/);
    for (const arch of manifest.architectures) {
      requireThat(architectures.includes(arch), `${basename(target)} is missing architecture ${arch}`);
    }
  }
  const rpaths = run("otool", ["-l", executable]);
  requireThat(rpaths.includes("@executable_path/../Frameworks"), "app executable lacks bundled Frameworks rpath");
  const runtimeLinks = run("otool", ["-L", join(frameworks, "libmpv.dylib")]);
  const ffmpegDependency = runtimeLinks.split("\n")
    .map((line) => line.trim().split(" (")[0])
    .find((reference) => reference.endsWith("/libavcodec.dylib"));
  requireThat(ffmpegDependency && isPackagedDylibReference(ffmpegDependency),
    `bundled libmpv does not use a packaged FFmpeg dependency: ${ffmpegDependency || "none"}`);
  return executable;
}

function start(executable, args, options) {
  const child = spawn(executable, args, options);
  let result;
  const closed = new Promise((done) => {
    child.once("error", (error) => { result = error; done(result); });
    child.once("close", (code, signal) => {
      result ??= code === 0 ? 0 : new Error(`app exit ${code ?? signal}`);
      done(result);
    });
  });
  return { child, closed, result: () => result };
}

async function waitForClose(process, milliseconds) {
  let timer;
  try {
    return await Promise.race([process.closed, new Promise((done) => {
      timer = setTimeout(() => done(new Error("app did not exit before the Gate deadline")), milliseconds);
    })]);
  } finally {
    clearTimeout(timer);
  }
}

async function verifyHandoff(executable) {
  const gate = mkdtempSync(join(tmpdir(), "echo-macos-gate-"));
  const openedLog = join(gate, "opened.log");
  const fixture = join(gate, "gate-tone-short.mp3");
  const home = join(gate, "home");
  const privateData = join(gate, "data");
  let first;
  let second;
  try {
    copyFileSync(FIXTURE, fixture);
    mkdirSync(home);
    mkdirSync(privateData);
    const env = {
      ...process.env,
      HOME: home,
      CFFIXED_USER_HOME: home,
      ECHO_GATE_DATA_DIR: privateData,
      ECHO_GATE_OPEN_LOG: openedLog,
      ECHO_GATE_QUIT_AFTER_MS: "20000",
    };
    const options = { cwd: gate, env, stdio: "ignore" };
    first = start(executable, [], options);
    await sleep(4000);
    requireThat(first.result() === undefined, `cold app exited early: ${first.result()}`);
    second = start(executable, [fixture], options);
    const hot = await waitForClose(second, 8000);
    requireThat(hot === 0, `hot start did not hand off to the running instance: ${hot}`);
    const deadline = Date.now() + 10000;
    for (;;) {
      const opened = existsSync(openedLog) ? readFileSync(openedLog, "utf8").trim().split(/\r?\n/) : [];
      if (opened.some((path) => path === fixture || path === realpathSync(fixture))) break;
      requireThat(first.result() === undefined && Date.now() <= deadline,
        "the launched bundle did not deliver the unique audio fixture to the running instance");
      await sleep(200);
    }
    const cold = await waitForClose(first, 24000);
    requireThat(cold === 0, `explicit Gate exit failed: ${cold}`);
  } finally {
    for (const attempt of [second, first]) {
      if (attempt && attempt.result() === undefined) {
        attempt.child.kill("SIGKILL");
        await waitForClose(attempt, 2000);
      }
    }
    rmSync(gate, { recursive: true, force: true });
  }
}

export async function inspectMacApp(bundle, { distIndex, run = readonlyCommand } = {}) {
  requireThat(process.platform === "darwin", "existing bundle checks require macOS");
  requireThat(bundle && basename(bundle) === "Echo.app", "an explicit Echo.app is required");
  const executable = validateIdentity(bundle, run);
  if (distIndex) process.stdout.write(run(process.execPath, [
    resolve(ROOT, "scripts/verify/checks/check-embedded-frontend.mjs"),
    "--strict", "--bin", executable, "--dist-index", distIndex,
  ]));
  await verifyHandoff(executable);
  console.log("ok macOS app: existing bundle associations, architectures, native links and live single-instance handoff");
}
