#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const args = process.argv.slice(2);
const targetIndex = args.indexOf("--target");
const targetArg = args.find((argument) => argument.startsWith("--target="));
const target =
  targetArg?.slice("--target=".length) ??
  (targetIndex >= 0 ? args[targetIndex + 1] : null);
const targetRoot = target
  ? resolve(ROOT, "target", target, "release")
  : resolve(ROOT, "target", "release");
const app = resolve(targetRoot, "bundle", "macos", "Echo.app");
const contents = resolve(app, "Contents");
const infoPlist = resolve(contents, "Info.plist");

function fail(message) {
  process.stderr.write(`FAIL macOS app signature: ${message}\n`);
  process.exit(1);
}

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: ROOT,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    fail(
      `${command} ${commandArgs.join(" ")} failed\n${result.stdout || ""}${result.stderr || ""}`,
    );
  }
  return `${result.stdout || ""}${result.stderr || ""}`;
}

if (process.platform !== "darwin") fail("this check only runs on macOS");
if (!existsSync(infoPlist)) fail(`missing ${infoPlist}`);

run("codesign", ["--verify", "--deep", "--strict", app]);
const bundleSignature = run("codesign", ["-dv", "--verbose=4", app]);
const codeDirectoryFlags = bundleSignature.match(/flags=[^\s]*\(([^)]*)\)/)?.[1] ?? "";
if (codeDirectoryFlags.split(",").includes("runtime")) {
  fail("ad-hoc app bundle enables hardened runtime, which blocks bundled libmpv library validation");
}
const bundleIdentifier = run("plutil", [
  "-extract",
  "CFBundleIdentifier",
  "raw",
  "-o",
  "-",
  infoPlist,
]).trim();
if (
  !bundleIdentifier ||
  !bundleSignature.includes(`Identifier=${bundleIdentifier}`)
) {
  fail(
    `signature identifier does not match CFBundleIdentifier (${bundleIdentifier || "missing"})`,
  );
}
if (bundleSignature.includes("Info.plist=not bound"))
  fail("bundle signature does not bind Info.plist");

const usageDescription = run("plutil", [
  "-extract",
  "NSLocalNetworkUsageDescription",
  "raw",
  "-o",
  "-",
  infoPlist,
]).trim();
if (!usageDescription) fail("missing NSLocalNetworkUsageDescription");
const bonjourServices = run("plutil", [
  "-extract",
  "NSBonjourServices",
  "json",
  "-o",
  "-",
  infoPlist,
]);
if (!bonjourServices.includes("_echo-sync._tcp"))
  fail("missing the Echo Bonjour service declaration");

process.stdout.write(`ok macOS app signature: ${bundleIdentifier} (${app})\n`);
