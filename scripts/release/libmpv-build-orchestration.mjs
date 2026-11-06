#!/usr/bin/env node
// I/O and build orchestration for the pinned macOS libmpv/FFmpeg audio build:
// downloads, extraction, configure/make/meson, otool-driven closure walking,
// install-name rewriting, codesign re-signing and the vendor writer. Every
// function here may touch the disk or spawn a process; importing this module
// has no side effects (the CLI guard lives in build-macos-libmpv.mjs).

import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { cpus, tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import {
  INSTALL_PREFIX,
  MACOSX_DEPLOYMENT_TARGET,
  MANIFEST_PATH,
  MPV_HARD_DEPENDENCIES,
  MPV_MESON_OPTIONS,
  FFMPEG_CONFIGURE_ARGS,
  AUXILIARY_COMPONENTS,
  NOTICE_PATH,
  PROBE_SCRIPT,
  ROOT,
  THIN_SCRIPT,
  VENDOR,
} from "./libmpv-build-config.mjs";
import {
  applyBuildRecords,
  assertHost,
  assertPatchResult,
  assertSafeScratchDir,
  classifyDependencyPath,
  deploymentTargetViolation,
  fail,
  findConfigureWarnings,
  findUnknownMesonOptions,
  homebrewVersionFromSource,
  mergeNoticeBlock,
  normalizeDylibName,
  parseArgs,
  parseMesonOptionNames,
  parseMinos,
  parseOtoolId,
  parseOtoolLibraries,
  planInstallNameChanges,
  renderNoticeBlock,
  rewritePkgConfigPrefix,
  verifyPinnedDigest,
} from "./libmpv-build-pure.mjs";
import {
  assertForbiddenLinkage,
  assertMinimalComponents,
  runProbe,
} from "./ffmpeg-component-probe.mjs";

function run(command, args, { cwd = ROOT, env, allowFailure = false, stdio } = {}) {
  const result = spawnSync(command, args, {
    cwd,
    env: env ?? process.env,
    encoding: "utf8",
    stdio: stdio ?? "pipe",
  });
  if (result.error) fail(`${command} failed to start: ${result.error.message}`);
  if (result.status !== 0 && !allowFailure) {
    const detail = `${result.stdout || ""}${result.stderr || ""}`.trim();
    fail(`${command} ${args.join(" ")} failed (exit ${result.status})${detail ? `\n${detail}` : ""}`);
  }
  return `${result.stdout || ""}${result.stderr || ""}`;
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function download(url, destination) {
  run("curl", ["-fL", "--retry", "3", "--fail", "-o", destination, url]);
}

function extractArchive(archive, destination) {
  mkdirSync(destination, { recursive: true });
  run("tar", ["-xf", archive, "-C", destination]);
}

export function resolvePatchPaths(additionalSources, root = ROOT) {
  // Patches are part of the pin set: a missing file must fail before any
  // download or compile, and the manifest order is the application order.
  // The SHA-256 is mandatory: a path alone would let an edited patch change
  // the build without changing the manifest.
  return (additionalSources ?? [])
    .filter((entry) => entry.path)
    .map((entry) => {
      const path = resolve(root, entry.path);
      if (!existsSync(path)) fail(`missing mpv patch: ${entry.path}`);
      verifyPinnedDigest({
        name: entry.path,
        expected: entry.sha256,
        actual: sha256File(path),
      });
      return { path, relativePath: entry.path, sha256: entry.sha256 };
    });
}

function assertPatchApplied(patchPath, cwd) {
  // --forward makes an already-applied patch fail instead of silently creating
  // rejects; the build must start from a clean extraction every time.
  const result = spawnSync("patch", ["-p1", "--batch", "--forward", "-i", patchPath], {
    cwd,
    encoding: "utf8",
  });
  assertPatchResult(result, basename(patchPath));
}

function buildEnv() {
  return { ...process.env, MACOSX_DEPLOYMENT_TARGET };
}

function pkgConfigEnv(ffmpegPcDir) {
  return { ...buildEnv(), PKG_CONFIG_PATH: ffmpegPcDir };
}

function buildFfmpeg({ work, sourceDir, log }) {
  const buildDir = resolve(work, "ffmpeg-build");
  const destDir = resolve(work, "ffmpeg-dest");
  mkdirSync(buildDir, { recursive: true });
  mkdirSync(destDir, { recursive: true });
  const configureLog = run(resolve(sourceDir, "configure"), FFMPEG_CONFIGURE_ARGS, {
    cwd: buildDir,
    env: buildEnv(),
  });
  writeFileSync(log, configureLog);
  const warnings = findConfigureWarnings(configureLog);
  if (warnings.length > 0) {
    fail(`FFmpeg configure did not match declared components:\n${warnings.join("\n")}`);
  }
  run("make", [`-j${Math.max(1, cpus().length)}`], {
    cwd: buildDir,
    stdio: "inherit",
    env: buildEnv(),
  });
  run("make", ["install", `DESTDIR=${destDir}`], {
    cwd: buildDir,
    stdio: "inherit",
    env: buildEnv(),
  });
  const pcDir = resolve(destDir, "usr/local/lib/pkgconfig");
  if (!existsSync(pcDir)) fail(`FFmpeg pkg-config directory missing: ${pcDir}`);
  for (const entry of readdirSync(pcDir)) {
    if (!entry.endsWith(".pc")) continue;
    const file = resolve(pcDir, entry);
    writeFileSync(
      file,
      rewritePkgConfigPrefix(readFileSync(file, "utf8"), resolve(destDir, "usr/local")),
    );
  }
  return { buildDir, destDir, libDir: resolve(destDir, "usr/local/lib"), pcDir };
}

function buildMpv({ work, sourceDir, ffmpegPcDir, log }) {
  const buildDir = resolve(work, "mpv-build");
  const destDir = resolve(work, "mpv-dest");
  mkdirSync(destDir, { recursive: true });
  const optionsSource = readFileSync(resolve(sourceDir, "meson_options.txt"), "utf8");
  const unknown = findUnknownMesonOptions(
    MPV_MESON_OPTIONS,
    parseMesonOptionNames(optionsSource),
  );
  if (unknown.length > 0) {
    fail(`mpv 0.36.0 does not declare meson option(s): ${unknown.join(", ")}`);
  }
  const configureLog = run(
    "meson",
    [
      "setup",
      buildDir,
      sourceDir,
      `--prefix=${INSTALL_PREFIX}`,
      "--buildtype=release",
      "--default-library=shared",
      ...MPV_MESON_OPTIONS,
    ],
    { cwd: work, env: pkgConfigEnv(ffmpegPcDir) },
  );
  writeFileSync(log, configureLog);
  run("ninja", ["-C", buildDir], { stdio: "inherit", env: buildEnv() });
  run("meson", ["install", "-C", buildDir, `--destdir=${destDir}`], {
    stdio: "inherit",
    env: buildEnv(),
  });
  return { buildDir, destDir, libDir: resolve(destDir, "usr/local/lib") };
}

function resolveDylibSource(reference, { mpvLibDir, ffmpegLibDir, homebrewPrefix }) {
  const name = reference.slice(reference.lastIndexOf("/") + 1);
  // Staging products take precedence over any absolute path recorded in the
  // install name. A staged FFmpeg dylib is named e.g.
  // /usr/local/lib/libavcodec.60.dylib, but the bytes for this build live in
  // the DESTDIR tree; a same-named library installed on the build host must
  // never win the lookup. Check mpvLibDir/ffmpegLibDir first, and consult the
  // literal absolute reference only as a last resort.
  const stagingCandidates = [resolve(mpvLibDir, name), resolve(ffmpegLibDir, name)];
  for (const candidate of stagingCandidates) {
    if (existsSync(candidate)) return realpathSync(candidate);
  }
  if (reference.startsWith("/") && existsSync(reference)) {
    return realpathSync(reference);
  }
  if (homebrewPrefix) {
    const optDir = resolve(homebrewPrefix, "opt");
    // Sort the opt enumeration so a host exposing both `freetype` and the
    // legacy `freetype2` symlink resolves deterministically. Collect every
    // match first: if distinct realpaths exist, fail instead of silently
    // depending on readdir order (which would change the vendor tree and the
    // NOTICE version depending on the host).
    const matches = [];
    for (const entry of readdirSync(optDir).sort()) {
      const candidate = resolve(optDir, entry, "lib", name);
      if (existsSync(candidate)) matches.push(realpathSync(candidate));
    }
    const distinct = [...new Set(matches)];
    if (distinct.length > 1) {
      fail(
        `${name} resolves to multiple distinct Homebrew dylibs (${distinct.join(", ")}); ` +
          "refusing to pick one by enumeration order",
      );
    }
    if (distinct.length === 1) return distinct[0];
  }
  return null;
}

function collectClosure({ out, mpvLibDir, ffmpegLibDir, homebrewPrefix }) {
  // Walk otool -L from libmpv.dylib, copying every non-system dependency into
  // the candidate directory under its release name. The graph is recorded at
  // the same time so install-name rewriting and the Gate share one view.
  mkdirSync(out, { recursive: true });
  const root = resolve(mpvLibDir, "libmpv.dylib");
  if (!existsSync(root)) fail(`libmpv.dylib was not installed in ${mpvLibDir}`);
  const sourceByTarget = new Map();
  const referencesByFile = {};
  const queue = [{ source: realpathSync(root), target: "libmpv.dylib" }];
  const systemRefs = [];
  while (queue.length > 0) {
    const { source, target } = queue.shift();
    if (sourceByTarget.has(target)) continue;
    sourceByTarget.set(target, source);
    const refs = parseOtoolLibraries(run("otool", ["-L", source]));
    referencesByFile[target] = [];
    const selfName = basename(source);
    for (const ref of refs) {
      // otool -L prints the LC_ID_DYLIB first. In the staging tree that ID is
      // an absolute /usr/local/lib path that cannot be classified, so drop the
      // self reference here; every dependent still gets its own rewrite.
      if (basename(ref) === selfName) continue;
      const classification = classifyDependencyPath(ref, {
        homebrewPrefix,
        // mpv links the staged FFmpeg through the rewritten .pc, but the
        // recorded install names stay at the configure prefix (/usr/local/lib).
        // Those references are staging references even though the bytes live
        // under DESTDIR; resolveDylibSource still fails if the file is absent.
        stagingPrefixes: [
          ffmpegLibDir,
          mpvLibDir,
          resolve(ffmpegLibDir, ".."),
          resolve(mpvLibDir, ".."),
          INSTALL_PREFIX,
        ],
      });
      if (classification.kind === "system") {
        systemRefs.push({ file: target, reference: ref });
        continue;
      }
      if (classification.kind === "unknown") {
        fail(`${target} references an unclassified dependency: ${ref}`);
      }
      const resolved = resolveDylibSource(ref, { mpvLibDir, ffmpegLibDir, homebrewPrefix });
      if (!resolved) fail(`${target} dependency cannot be resolved on this host: ${ref}`);
      // Name the candidate from the reference (the library's soname), not the
      // realpath: Homebrew ships libgraphite2.3.dylib as a symlink to
      // libgraphite2.3.3.1.dylib, and the vendor convention keeps the soname.
      const normalizedTarget = normalizeDylibName(ref.slice(ref.lastIndexOf("/") + 1));
      referencesByFile[target].push(ref);
      if (!sourceByTarget.has(normalizedTarget)) {
        queue.push({ source: resolved, target: normalizedTarget });
      }
    }
  }
  return { sourceByTarget, referencesByFile, systemRefs };
}

function copyAndRewrite({ out, sourceByTarget, referencesByFile }) {
  const files = [...sourceByTarget.keys()].sort();
  for (const file of files) {
    copyFileSync(sourceByTarget.get(file), resolve(out, file));
    // Homebrew dylibs install 0444. Keep the candidate writable so the
    // downstream thin/strip step does not fail on a read-only mode.
    chmodSync(resolve(out, file), 0o644);
  }
  const plan = planInstallNameChanges({ files, referencesByFile });
  if (plan.unknownReferences.length > 0) {
    fail(
      `candidate has unresolved dependencies:\n${plan.unknownReferences
        .map((entry) => `${entry.file}: ${entry.reference}`)
        .join("\n")}`,
    );
  }
  for (const { file, installName } of plan.ids) {
    run("install_name_tool", ["-id", installName, resolve(out, file)]);
  }
  for (const { file, from, to } of plan.changes) {
    run("install_name_tool", ["-change", from, to, resolve(out, file)]);
  }
  // install_name_tool exits 0 even when `-change <from>` names a load command
  // that is not present, silently leaving a build-host absolute path behind.
  // Re-read the resulting dylibs with otool -D / -L and prove the ID is exactly
  // the planned value and every planned `from` is gone while its `to` is
  // present, before any signature is (re)applied. A mismatch is a hard failure.
  const idsByFile = new Map(plan.ids.map(({ file, installName }) => [file, installName]));
  const changesByFile = new Map();
  for (const { file, from, to } of plan.changes) {
    if (!changesByFile.has(file)) changesByFile.set(file, []);
    changesByFile.get(file).push({ from, to });
  }
  for (const file of files) {
    const path = resolve(out, file);
    const ids = parseOtoolId(run("otool", ["-D", path]));
    if (ids.length !== 1 || ids[0] !== idsByFile.get(file)) {
      fail(
        `${file}: after rewriting, LC_ID_DYLIB is [${ids.join(", ")}], expected exactly ${idsByFile.get(file)}`,
      );
    }
    const refs = new Set(parseOtoolLibraries(run("otool", ["-L", path])));
    for (const { from, to } of changesByFile.get(file) ?? []) {
      if (refs.has(from) || !refs.has(to)) {
        fail(
          `${file}: install_name_tool did not apply -change ${from} -> ${to} ` +
            `(from still present: ${refs.has(from)}, to present: ${refs.has(to)})`,
        );
      }
    }
  }
  // install_name_tool invalidates the arm64 code signature; every modified
  // dylib must be ad-hoc re-signed before the probe can load it.
  for (const file of files) {
    run("codesign", ["--force", "--sign", "-", resolve(out, file)]);
  }
  return files;
}

function readHomebrewVersions(files, sourceByTarget) {
  const byFile = new Map(AUXILIARY_COMPONENTS.map((entry) => [entry.file, entry]));
  return files
    .filter((file) => !/^lib(avcodec|avformat|avutil|avfilter|swresample|swscale|mpv)\.dylib$/.test(file))
    .sort()
    .map((file) => {
      const mapping = byFile.get(file);
      if (!mapping) {
        fail(
          `unmapped auxiliary dylib ${file}; add it to AUXILIARY_COMPONENTS with its Homebrew formula before publishing`,
        );
      }
      const version = homebrewVersionFromSource(sourceByTarget.get(file), mapping.formula);
      return { component: mapping.component, version, files: [file] };
    });
}

function readToolchain(sourceDir) {
  const clang = run("cc", ["--version"]).split("\n")[0].trim();
  const sdk = run("xcrun", ["--show-sdk-version"]).trim();
  return `${clang}; SDK ${sdk}`;
}

function readMinos(file) {
  const minos = parseMinos(run("otool", ["-l", file]));
  return minos ?? "unknown";
}

function build(argv) {
  const opts = parseArgs(argv);
  if (opts.help) {
    process.stdout.write(
      "usage: build-macos-libmpv.mjs --work <dir> --out <dir>\n" +
        "       build-macos-libmpv.mjs --out <dir> --write-vendor\n",
    );
    return;
  }
  assertHost(process.platform, process.arch);
  const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
  if (opts.writeVendor) {
    writeVendor(manifest, opts.out);
    return;
  }

  const work = assertSafeScratchDir(opts.work);
  const out = assertSafeScratchDir(opts.out);
  if (work === out) fail("--work and --out must be different directories");
  rmSync(work, { recursive: true, force: true });
  rmSync(out, { recursive: true, force: true });
  mkdirSync(work, { recursive: true });
  mkdirSync(out, { recursive: true });

  const ffmpegArchive = resolve(work, "ffmpeg.tar.xz");
  const mpvArchive = resolve(work, "mpv.tar.gz");
  download(manifest.sourceUrl, ffmpegArchive);
  verifyPinnedDigest({
    name: basename(ffmpegArchive),
    expected: manifest.assetSha256,
    actual: sha256File(ffmpegArchive),
  });
  const mpvSource = manifest.additionalSources.find((entry) => entry.name === "mpv");
  if (!mpvSource?.url) fail("manifest has no mpv source URL");
  download(mpvSource.url, mpvArchive);
  verifyPinnedDigest({
    name: basename(mpvArchive),
    expected: mpvSource.sha256,
    actual: sha256File(mpvArchive),
  });

  const extracted = resolve(work, "src");
  extractArchive(ffmpegArchive, extracted);
  extractArchive(mpvArchive, extracted);
  const ffmpegSource = resolve(extracted, `ffmpeg-${manifest.builds?.ffmpeg?.version ?? "6.0"}`);
  const mpvSourceDir = resolve(extracted, `mpv-${manifest.builds?.mpv?.version ?? "0.36.0"}`);
  if (!existsSync(resolve(ffmpegSource, "configure"))) fail("ffmpeg source tree not found after extraction");
  if (!existsSync(resolve(mpvSourceDir, "meson_options.txt"))) fail("mpv source tree not found after extraction");
  const patches = resolvePatchPaths(manifest.additionalSources);
  for (const patch of patches) {
    assertPatchApplied(patch.path, mpvSourceDir);
  }

  const ffmpeg = buildFfmpeg({
    work,
    sourceDir: ffmpegSource,
    log: resolve(work, "ffmpeg-configure.log"),
  });
  const mpv = buildMpv({
    work,
    sourceDir: mpvSourceDir,
    ffmpegPcDir: ffmpeg.pcDir,
    log: resolve(work, "mpv-meson.log"),
  });
  const homebrewPrefix = run("brew", ["--prefix"]).trim();
  const { sourceByTarget, referencesByFile } = collectClosure({
    out,
    mpvLibDir: mpv.libDir,
    ffmpegLibDir: ffmpeg.libDir,
    homebrewPrefix,
  });
  for (const dependency of MPV_HARD_DEPENDENCIES) {
    if (!sourceByTarget.has(dependency)) {
      fail(
        `mpv hard dependency ${dependency} is missing from the dependency closure; ` +
          "refusing to publish a libmpv that cannot load",
      );
    }
  }
  const files = copyAndRewrite({ out, sourceByTarget, referencesByFile });
  let forbiddenLinks = 0;
  for (const file of files) {
    forbiddenLinks += assertForbiddenLinkage(run("otool", ["-L", resolve(out, file)])).length;
    const targetViolation = deploymentTargetViolation(file, readMinos(resolve(out, file)));
    if (targetViolation) {
      fail(`${targetViolation}; the build inherited the host deployment target`);
    }
  }

  const probe = runProbe(out);
  const probeVerdict = assertMinimalComponents(probe, {
    expectedVersion: manifest.builds?.ffmpeg?.version ?? "6.0",
    expectedConfigure: FFMPEG_CONFIGURE_ARGS,
  });
  const auxiliary = readHomebrewVersions(files, sourceByTarget);
  const provenance = {
    schemaVersion: 1,
    buildScript: "scripts/release/build-macos-libmpv.mjs",
    builds: {
      ffmpeg: {
        version: manifest.builds?.ffmpeg?.version ?? "6.0",
        configure: [...FFMPEG_CONFIGURE_ARGS],
        deploymentTarget: MACOSX_DEPLOYMENT_TARGET,
        toolchain: readToolchain(ffmpegSource),
      },
      mpv: {
        version: manifest.builds?.mpv?.version ?? "0.36.0",
        meson: [...MPV_MESON_OPTIONS],
        patches: patches.map((patch) => patch.relativePath),
        hardDependencies: [...MPV_HARD_DEPENDENCIES],
      },
      auxiliary,
    },
    buildHost: `macOS ${run("sw_vers", ["-productVersion"]).trim()} arm64; Homebrew ${homebrewPrefix}; auxiliary versions recorded in NOTICE.md`,
    files,
    probe: { counts: probeVerdict.counts, forbiddenLinkage: forbiddenLinks },
  };
  writeFileSync(resolve(out, "provenance.json"), `${JSON.stringify(provenance, null, 2)}\n`);
  process.stdout.write(
    `Built ${files.length} dylib(s) in ${out}; probe counts ${JSON.stringify(probeVerdict.counts)}\n`,
  );
}

export function backupVendorTree(vendorDir) {
  const backupDir = mkdtempSync(resolve(tmpdir(), "echo-vendor-backup-"));
  for (const entry of readdirSync(vendorDir)) {
    if (!entry.endsWith(".dylib") && entry !== "manifest.json" && entry !== "NOTICE.md") continue;
    cpSync(resolve(vendorDir, entry), resolve(backupDir, entry));
  }
  return backupDir;
}

export function restoreVendorTree(vendorDir, backupDir) {
  for (const entry of readdirSync(vendorDir)) {
    if (entry.endsWith(".dylib")) rmSync(resolve(vendorDir, entry), { force: true });
  }
  for (const entry of readdirSync(backupDir)) {
    cpSync(resolve(backupDir, entry), resolve(vendorDir, entry));
  }
}

function writeVendor(manifest, out) {
  const provenancePath = resolve(out, "provenance.json");
  if (!existsSync(provenancePath)) fail(`candidate provenance missing: ${provenancePath}`);
  const provenance = JSON.parse(readFileSync(provenancePath, "utf8"));
  const files = [...provenance.files].sort();
  for (const file of files) {
    if (!existsSync(resolve(out, file))) fail(`candidate file missing: ${file}`);
  }
  const pinnedPatches = (manifest.additionalSources ?? [])
    .filter((entry) => entry.path)
    .map((entry) => entry.path);
  const recordedPatches = provenance.builds?.mpv?.patches ?? [];
  if (JSON.stringify(recordedPatches) !== JSON.stringify(pinnedPatches)) {
    fail(
      `candidate provenance patches [${recordedPatches.join(", ")}] do not match the manifest pins ` +
        `[${pinnedPatches.join(", ")}]`,
    );
  }

  const backupDir = backupVendorTree(VENDOR);
  let keepBackup = false;
  try {
    for (const entry of readdirSync(VENDOR)) {
      if (!entry.endsWith(".dylib")) continue;
      if (!files.includes(entry)) rmSync(resolve(VENDOR, entry), { force: true });
    }
    const fileHashes = {};
    for (const file of files) {
      const destination = resolve(VENDOR, file);
      // A previous vendor copy may be read-only, and copyFileSync cannot
      // overwrite it in place.
      rmSync(destination, { force: true });
      copyFileSync(resolve(out, file), destination);
      chmodSync(destination, 0o644);
      fileHashes[file] = sha256File(destination);
    }
    const next = applyBuildRecords(manifest, {
      files,
      fileHashes,
      builds: provenance.builds,
      buildHost: provenance.buildHost,
    });
    writeFileSync(MANIFEST_PATH, `${JSON.stringify(next, null, 2)}\n`);
    const notice = readFileSync(NOTICE_PATH, "utf8");
    writeFileSync(
      NOTICE_PATH,
      mergeNoticeBlock(notice, renderNoticeBlock(provenance.builds.auxiliary)),
    );
    process.stdout.write(`Wrote ${files.length} dylib(s) and provenance to ${VENDOR}\n`);
    run(process.execPath, [THIN_SCRIPT, "--write"], { stdio: "inherit" });
    run(process.execPath, [PROBE_SCRIPT, "--vendor", VENDOR], { stdio: "inherit" });
  } catch (error) {
    try {
      restoreVendorTree(VENDOR, backupDir);
    } catch (restoreError) {
      keepBackup = true;
      fail(
        `vendor write failed (${error.message}) and rollback also failed (${restoreError.message}); ` +
          `the previous tree is preserved at ${backupDir}`,
      );
    }
    fail(`vendor write failed and was rolled back: ${error.message}`);
  } finally {
    if (!keepBackup) rmSync(backupDir, { recursive: true, force: true });
  }
}

export { build, writeVendor };
