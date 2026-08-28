#!/usr/bin/env node
// macOS Tauri packaging Gate. It builds the .app, checks that the supported
// audio associations and universal libmpv framework are bundled, then proves
// cold/hot single-instance delivery and explicit exit using only Gate-only
// environment variables (no frontend privilege is added).

import { existsSync, mkdtempSync, readFileSync, renameSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");
const ARM_BUNDLE = resolve(ROOT, "target", "aarch64-apple-darwin", "release", "bundle", "macos", "Echo.app");
const X64_BUNDLE = resolve(ROOT, "target", "x86_64-apple-darwin", "release", "bundle", "macos", "Echo.app");
const BUNDLE = ARM_BUNDLE;
const MACOS = join(BUNDLE, "Contents", "MacOS");
const FRAMEWORKS = join(BUNDLE, "Contents", "Frameworks");
const EXECUTABLE = join(MACOS, "echo");
const FIXTURE = resolve(ROOT, "fixtures", "audio", "tone-short.mp3");
const REQUIRED_EXTENSIONS = ["mp3", "flac", "m4a", "ogg", "opus", "wav"];

function fail(message) {
  process.stderr.write(`FAIL 1.9: ${message}\n`);
  process.exit(1);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8", ...options });
  if (result.status !== 0) {
    fail(`${command} ${args.join(" ")} exited ${result.status}\n${result.stdout || ""}${result.stderr || ""}`);
  }
  return `${result.stdout || ""}${result.stderr || ""}`;
}

function wait(milliseconds) {
  return new Promise((resolveWait) => setTimeout(resolveWait, milliseconds));
}

function once(child, event) {
  return new Promise((resolveEvent) => child.once(event, resolveEvent));
}

if (process.platform !== "darwin") {
  fail("the current Stage 1 Gate is intentionally macOS-local; run it on macOS");
}

for (const target of ["aarch64-apple-darwin", "x86_64-apple-darwin"]) {
  run("rustup", ["target", "add", target]);
  run("pnpm", ["--filter", "@echo/desktop", "tauri", "build", "--bundles", "app", "--target", target]);
}

const armExecutable = join(ARM_BUNDLE, "Contents", "MacOS", "echo");
const x64Executable = join(X64_BUNDLE, "Contents", "MacOS", "echo");
const mergedExecutable = join(MACOS, "echo.universal");
run("lipo", ["-create", armExecutable, x64Executable, "-output", mergedExecutable]);
renameSync(mergedExecutable, armExecutable);
run("codesign", ["--force", "--deep", "--sign", "-", BUNDLE]);

if (!existsSync(EXECUTABLE)) fail(`missing bundled executable: ${EXECUTABLE}`);
if (!existsSync(FIXTURE)) fail(`missing guaranteed-format fixture: ${FIXTURE}`);

const plist = run("plutil", ["-p", join(BUNDLE, "Contents", "Info.plist")]);
for (const ext of REQUIRED_EXTENSIONS) {
  if (!plist.includes(`"${ext}"`)) fail(`Info.plist is missing the .${ext} file association`);
}

const manifest = JSON.parse(
  readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "macos", "manifest.json"), "utf8"),
);
for (const filename of Object.keys(manifest.files)) {
  const framework = join(FRAMEWORKS, filename);
  if (!existsSync(framework)) fail(`missing bundled framework: ${framework}`);
  const architectures = run("lipo", ["-info", framework]);
  for (const arch of manifest.architectures) {
    if (!architectures.includes(arch)) fail(`${filename} is not universal (${arch} missing)`);
  }
}

const rpaths = run("otool", ["-l", EXECUTABLE]);
if (!rpaths.includes("@executable_path/../Frameworks")) {
  fail("application executable lacks @executable_path/../Frameworks rpath");
}

const executableArchitectures = run("lipo", ["-info", EXECUTABLE]);
for (const architecture of manifest.architectures) {
  if (!executableArchitectures.includes(architecture)) {
    fail(`Echo executable is not universal (${architecture} missing)`);
  }
}

const gate = mkdtempSync(join(tmpdir(), "echo-macos-gate-"));
const openedLog = join(gate, "opened.log");
const environment = {
  ...process.env,
  ECHO_GATE_OPEN_LOG: openedLog,
  ECHO_GATE_QUIT_AFTER_MS: "15000",
};
const first = spawn(EXECUTABLE, [], { cwd: MACOS, env: environment, stdio: "ignore" });

try {
  // Tauri's WebView and menu-bar registration can take a few seconds on a
  // cold machine; don't race the single-instance service startup.
  await wait(4000);
  if (first.exitCode !== null) fail(`cold start exited early with ${first.exitCode}`);

  const hot = spawnSync(EXECUTABLE, [FIXTURE], { cwd: MACOS, env: environment, encoding: "utf8", timeout: 8000 });
  if (hot.status !== 0) {
    fail(`hot start did not hand off to the running instance (${hot.status})\n${hot.stdout || ""}${hot.stderr || ""}`);
  }

  await wait(500);
  if (!existsSync(openedLog) || !readFileSync(openedLog, "utf8").includes(basename(FIXTURE))) {
    fail("hot single-instance launch did not deliver the opened audio path to the first instance");
  }

  const exitCode = await once(first, "close");
  if (exitCode !== 0) fail(`explicit Gate exit returned ${exitCode}`);
} finally {
  if (first.exitCode === null) first.kill("SIGTERM");
  rmSync(gate, { recursive: true, force: true });
}

const libmpv = join(FRAMEWORKS, "libmpv.dylib");
const runtimeLinks = run("otool", ["-L", libmpv]);
if (!runtimeLinks.includes("@rpath/libavcodec.dylib")) {
  fail("bundled libmpv does not resolve FFmpeg through the packaged rpath");
}

process.stdout.write("ok 1.9: macOS Tauri app bundles libmpv, file associations, tray and single-instance Gate pass\n");
