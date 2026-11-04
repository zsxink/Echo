#!/usr/bin/env node
// Audit, thin and strip Echo's vendored macOS libmpv stack to the release
// target.
//
// The release target is arm64-only (manifest `architectures` and the
// ci-release-pipeline contract). Rebuilds used to land universal dylibs, which
// shipped an x86_64 slice nobody can load. Local symbols add another ~1.2 MB
// of __LINKEDIT that the loader never needs. This tool makes the accepted
// vendor state explicit and reproducible:
//
//   node scripts/release/thin-macos-libmpv.mjs           # dry run, no writes
//   node scripts/release/thin-macos-libmpv.mjs --write   # thin + strip + re-pin sha256
//
// The Gate (task 9.7 / 1.10) imports the same pure comparison functions, so
// "what the tool considers correct" and "what the release enforces" cannot
// drift apart.

import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const VENDOR = resolve(
  ROOT,
  "apps",
  "desktop",
  "src-tauri",
  "vendor",
  "libmpv",
  "macos",
);
const MANIFEST_PATH = resolve(VENDOR, "manifest.json");

export function parseLipoArchs(output) {
  const archs = String(output ?? "")
    .trim()
    .split(/\s+/)
    .filter(Boolean);
  if (archs.length === 0) {
    throw new Error("lipo -archs produced no architecture list");
  }
  return [...new Set(archs)].sort();
}

export function classifyArchitectures(actual, expected) {
  if (!Array.isArray(expected) || expected.length === 0) {
    throw new Error("manifest declares no expected architectures");
  }
  const actualSet = new Set(actual);
  const expectedSet = new Set(expected);
  const missing = [...expectedSet].filter((arch) => !actualSet.has(arch)).sort();
  const extra = [...actualSet].filter((arch) => !expectedSet.has(arch)).sort();
  if (missing.length > 0) return { status: "missing", missing, extra };
  if (extra.length > 0) return { status: "thin", missing: [], extra };
  return { status: "match", missing: [], extra: [] };
}

export function planThinning(manifest, actualArchitectures) {
  return Object.keys(manifest.files).map((filename) => {
    // The target contract is the release architecture set, not the per-file
    // record: fileArchitectures describes what a rebuild actually produced, so
    // trusting it here would treat a fat rebuild as correct.
    const expected = manifest.architectures;
    const actual = actualArchitectures[filename] ?? [];
    return {
      filename,
      expected: [...expected].sort(),
      actual: [...actual].sort(),
      ...classifyArchitectures(actual, expected),
    };
  });
}

export function planVendorSteps(entry) {
  // `strip -x` must run before re-signing: it rewrites __LINKEDIT and
  // invalidates any existing signature. `lipo -thin` is only needed for files
  // that still carry extra slices; stripping already-stripped dylibs is a
  // byte-for-byte no-op, so every manifest file goes through the same pipeline.
  const steps = [];
  if (entry.status === "thin") {
    if (entry.expected.length !== 1) {
      throw new Error(
        `${entry.filename} declares ${entry.expected.length} architectures; thinning supports a single target architecture`,
      );
    }
    steps.push({ command: "lipo", args: ["-thin", entry.expected[0]] });
  } else if (entry.status !== "match") {
    throw new Error(
      `${entry.filename} has status ${entry.status}; rebuild the baseline instead of repairing it in place`,
    );
  }
  steps.push({ command: "chmod" });
  steps.push({ command: "otool", args: ["-D"] });
  steps.push({ command: "strip", args: ["-x"] });
  steps.push({ command: "codesign", args: ["--force", "--sign", "-"] });
  return steps;
}

export function countLocalSymbols(nmOutput) {
  // Lowercase nm types are local symbols (t/s text, b/d data). External and
  // undefined symbols are uppercase (T/S/U) and must remain for dynamic
  // linking, so a stripped dylib legitimately reports zero of these. Each nm
  // line is "<address> <type> <name>", with the address optional for some
  // symbol kinds; only a bare lowercase type token counts.
  return String(nmOutput ?? "")
    .split("\n")
    .filter((line) => /^\s*(?:[0-9a-f]+\s+)?[tsbd]\s/.test(line))
    .length;
}

export function parseDylibReferences(otoolOutput) {
  // `otool -L` prints the inspected path on the first line, then one reference
  // per line. Only flat @rpath/@loader_path dylib names resolve inside the
  // app bundle; system frameworks and absolute Homebrew paths do not.
  return String(otoolOutput ?? "")
    .split("\n")
    .slice(1)
    .map((line) => line.trim().split(/\s+/)[0])
    .filter((ref) => isPackagedDylibReference(ref))
    .map((ref) => ref.slice(ref.lastIndexOf("/") + 1));
}

export function computeVendorClosure(rootFile, referencesByFile) {
  // Walk the packaged dylib graph from libmpv.dylib. A file missing from the
  // map is still reported, so a broken reference fails closed instead of
  // silently shrinking the closure.
  const seen = new Set();
  const stack = [rootFile];
  while (stack.length > 0) {
    const filename = stack.pop();
    if (seen.has(filename)) continue;
    seen.add(filename);
    for (const ref of referencesByFile[filename] ?? []) stack.push(ref);
  }
  return [...seen].sort();
}

export function compareVendorSets({ manifestFiles, closure, bundleFrameworks }) {
  // The three views of "what ships" must be the same set. Return human-readable
  // differences so the Gate can name the offending file rather than just fail.
  const manifest = new Set(manifestFiles);
  const dep = new Set(closure);
  const bundle = new Set(bundleFrameworks);
  const missingFromManifest = [...dep].filter((f) => !manifest.has(f)).sort();
  const unusedInManifest = [...manifest].filter((f) => !dep.has(f)).sort();
  const missingFromBundle = [...manifest].filter((f) => !bundle.has(f)).sort();
  const extraInBundle = [...bundle].filter((f) => !manifest.has(f)).sort();
  return { missingFromManifest, unusedInManifest, missingFromBundle, extraInBundle };
}

export function vendorSetsMatch(comparison) {
  return Object.values(comparison).every((list) => list.length === 0);
}

export function findHashMismatches(manifest, actualHashes) {
  const mismatches = [];
  for (const [filename, expected] of Object.entries(manifest.files)) {
    const actual = actualHashes[filename];
    if (actual !== expected) {
      mismatches.push({ filename, expected, actual: actual ?? null });
    }
  }
  return mismatches;
}

export function applyVendorState(manifest, entries) {
  const next = JSON.parse(JSON.stringify(manifest));
  next.fileArchitectures ??= {};
  for (const { filename, architectures, sha256 } of entries) {
    if (!Object.hasOwn(next.files, filename)) {
      throw new Error(`unknown vendor file: ${filename}`);
    }
    next.fileArchitectures[filename] = [...architectures].sort();
    next.files[filename] = sha256;
  }
  return next;
}

export function parseInstallNames(output) {
  // `otool -D` prefixes output with the inspected path and, for a universal
  // binary, repeats the install name once per architecture slice. The compare
  // is per name set, not per output line.
  return [
    ...new Set(
      String(output ?? "")
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line.startsWith("@rpath/")),
    ),
  ].sort();
}

export function isPackagedDylibReference(reference) {
  // Every framework dylib lands flat in Contents/Frameworks. Both placeholders
  // resolve there: @loader_path against the referencing dylib's own directory,
  // @rpath through the executable's @executable_path/../Frameworks rpath. A
  // bare or absolute name, a subdirectory, or a parent traversal would escape
  // the bundle, so only flat placeholder-relative names count as packaged.
  return /^@(rpath|loader_path)\/[^/]+\.dylib$/.test(String(reference ?? "").trim());
}

function run(command, args, { allowFailure = false } = {}) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.error) {
    throw new Error(
      `${command} failed to start: ${result.error.message}; this tool requires macOS lipo/codesign/otool`,
    );
  }
  if (result.status !== 0 && !allowFailure) {
    const detail = `${result.stdout || ""}${result.stderr || ""}`.trim();
    throw new Error(
      `${command} ${args.join(" ")} failed (exit ${result.status})${detail ? `\n${detail}` : ""}`,
    );
  }
  return {
    status: result.status,
    stdout: result.stdout || "",
    stderr: result.stderr || "",
  };
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function mb(bytes) {
  return `${(bytes / 1048576).toFixed(2)} MB`;
}

function installNames(path) {
  return parseInstallNames(run("otool", ["-D", path]).stdout).join("\n");
}

function signatureValid(path) {
  return run("codesign", ["--verify", "--strict", path], { allowFailure: true })
    .status === 0;
}

function ensureSignature(path) {
  if (signatureValid(path)) return;
  // Vendor dylibs are ad-hoc signed; the manifest SHA-256 is the supply-chain
  // pin, and packaging re-signs the bundle. Re-signing after thinning is safe.
  run("codesign", ["--force", "--sign", "-", path]);
  if (!signatureValid(path)) {
    throw new Error(`signature is invalid after thinning: ${basename(path)}`);
  }
}

function measureThinSize(path, arch, scratch) {
  const output = resolve(scratch, `${basename(path)}.${arch}`);
  run("lipo", ["-thin", arch, path, "-output", output]);
  const size = statSync(output).size;
  rmSync(output, { force: true });
  return size;
}

function prepareVendorCopy(entry) {
  const file = resolve(VENDOR, entry.filename);
  const tmp = `${file}.thin-${process.pid}`;
  const steps = planVendorSteps(entry);
  try {
    const before = installNames(file);
    let haveCopy = false;
    for (const step of steps) {
      switch (step.command) {
        case "lipo":
          run("lipo", ["-thin", ...step.args, file, "-output", tmp]);
          haveCopy = true;
          break;
        case "chmod":
          // Every file is prepared through a temp copy, so strip never touches
          // the original. A lipo output needs its mode restored; copying keeps
          // it via copyFileSync below.
          if (!haveCopy) {
            copyFileSync(file, tmp);
            haveCopy = true;
          }
          chmodSync(tmp, statSync(file).mode);
          break;
        case "otool": {
          const target = haveCopy ? tmp : file;
          if (installNames(target) !== before) {
            throw new Error(`install-name drift after thinning: ${entry.filename}`);
          }
          break;
        }
        case "strip":
          run("strip", [...step.args, tmp]);
          break;
        case "codesign":
          ensureSignature(tmp);
          break;
        default:
          throw new Error(`unknown vendor step: ${step.command}`);
      }
    }
  } catch (error) {
    rmSync(tmp, { force: true });
    throw error;
  }
  return { filename: entry.filename, file, tmp };
}

function printAudit(manifest, plan, mismatches, projectedSavings) {
  process.stdout.write(
    `macOS libmpv vendor audit (target: ${manifest.architectures.join("+")})\n`,
  );
  for (const entry of plan) {
    const actual = entry.actual.join("+") || "none";
    if (entry.status === "match") {
      process.stdout.write(`  ok    ${entry.filename.padEnd(24)} ${actual}\n`);
      continue;
    }
    if (entry.status === "thin") {
      process.stdout.write(
        `  thin  ${entry.filename.padEnd(24)} ${actual} -> ${entry.expected.join("+")}\n`,
      );
      continue;
    }
    process.stdout.write(
      `  FAIL  ${entry.filename.padEnd(24)} missing ${entry.missing.join("+")}; actual ${actual}\n`,
    );
  }
  for (const mismatch of mismatches) {
    process.stdout.write(
      `  FAIL  ${mismatch.filename.padEnd(24)} SHA-256 ${mismatch.expected} -> ${mismatch.actual ?? "unreadable"}\n`,
    );
  }
  const thinned = plan.filter((entry) => entry.status === "thin").length;
  const matched = plan.filter((entry) => entry.status === "match").length;
  const missing = plan.filter((entry) => entry.status === "missing").length;
  process.stdout.write(
    `Summary: ${matched} match, ${thinned} need thinning, ${missing} missing, ` +
      `${mismatches.length} hash mismatch(es); projected savings ${mb(projectedSavings)}\n`,
  );
}

function main() {
  const write = process.argv.includes("--write");
  if (process.platform !== "darwin") {
    throw new Error("this tool requires macOS (lipo/codesign/otool)");
  }
  if (!existsSync(MANIFEST_PATH)) {
    throw new Error(`manifest not found: ${MANIFEST_PATH}`);
  }
  const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
  const actualArchitectures = {};
  const actualHashes = {};
  const sizes = {};

  for (const filename of Object.keys(manifest.files)) {
    const file = resolve(VENDOR, filename);
    if (!existsSync(file)) {
      throw new Error(`vendor file missing: ${filename}`);
    }
    actualArchitectures[filename] = parseLipoArchs(
      run("lipo", ["-archs", file]).stdout,
    );
    actualHashes[filename] = sha256(file);
    sizes[filename] = statSync(file).size;
  }

  const plan = planThinning(manifest, actualArchitectures);
  const mismatches = findHashMismatches(manifest, actualHashes);
  const missing = plan.filter((entry) => entry.status === "missing");
  if (missing.length > 0) {
    printAudit(manifest, plan, mismatches, 0);
    throw new Error(
      `vendor files lack declared architectures: ${missing.map((e) => e.filename).join(", ")}; rebuild or replace the baseline instead of adding slices`,
    );
  }

  let projectedSavings = 0;
  const scratch = mkdtempSync(resolve(tmpdir(), "echo-thin-libmpv-"));
  try {
    for (const entry of plan) {
      if (entry.status !== "thin") continue;
      if (entry.expected.length !== 1) {
        throw new Error(
          `${entry.filename} declares ${entry.expected.length} architectures; thinning supports a single target architecture`,
        );
      }
      projectedSavings +=
        sizes[entry.filename] -
        measureThinSize(
          resolve(VENDOR, entry.filename),
          entry.expected[0],
          scratch,
        );
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }

  printAudit(manifest, plan, mismatches, projectedSavings);

  if (!write) {
    if (mismatches.length > 0) {
      throw new Error(
        "manifest SHA-256 does not match the vendor bytes; review the change, then re-run with --write to re-pin",
      );
    }
    const pending = plan.filter((entry) => entry.status === "thin").length;
    process.stdout.write(
      pending > 0
        ? "Dry run: re-run with --write to thin the vendor tree and update manifest.json.\n"
        : "No changes: vendor architectures and hashes match manifest.json.\n",
    );
    return;
  }

  const thinned = plan.filter((entry) => entry.status === "thin");
  const prepared = [];
  try {
    // Every manifest file goes through the same pipeline: thin only where
    // needed, then strip local symbols and re-sign. `strip -x` is a no-op on
    // already-stripped dylibs, so a run with no thinning still refreshes the
    // manifest hashes and enforces symbol purity on the whole tree.
    for (const entry of plan) {
      prepared.push(prepareVendorCopy(entry));
    }
    // All copies are verified before any original is replaced, so a failed
    // lipo/strip/codesign leaves the vendor tree untouched.
    for (const item of prepared) {
      renameSync(item.tmp, item.file);
    }
  } finally {
    for (const item of prepared) {
      rmSync(item.tmp, { force: true });
    }
  }

  const finalEntries = [];
  for (const filename of Object.keys(manifest.files)) {
    const file = resolve(VENDOR, filename);
    const actual = parseLipoArchs(run("lipo", ["-archs", file]).stdout);
    const check = classifyArchitectures(actual, manifest.architectures);
    if (check.status !== "match") {
      throw new Error(
        `${filename} is ${actual.join("+")} after thinning, expected ${manifest.architectures.join("+")}`,
      );
    }
    finalEntries.push({
      filename,
      architectures: actual,
      sha256: sha256(file),
    });
  }
  const next = applyVendorState(manifest, finalEntries);
  writeFileSync(MANIFEST_PATH, `${JSON.stringify(next, null, 2)}\n`);
  process.stdout.write(
    `Wrote ${MANIFEST_PATH}; thinned ${thinned.length} file(s), stripped ${plan.length} file(s), saved ${mb(projectedSavings)} from thinning.\n`,
  );
}

const invoked = process.argv[1] ? resolve(process.argv[1]) : "";
if (invoked === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`FAIL thin-macos-libmpv: ${error.message}\n`);
    process.exit(1);
  }
}
