#!/usr/bin/env node
// Self-test for thin-macos-libmpv.mjs's architecture and manifest logic.
//
// The load-bearing pieces run on every platform: the vendored Mach-O parser
// input, the exact-set comparison the Gate relies on, and the manifest rewrite
// that keeps SHA-256 pins in sync with the thinned bytes. The lipo/codesign
// calls themselves only run on macOS, so they are not exercised here; a fake
// `lipo` on PATH would test the fake, not the toolchain.

import {
  applyVendorState,
  classifyArchitectures,
  findHashMismatches,
  isPackagedDylibReference,
  parseInstallNames,
  parseLipoArchs,
  planThinning,
} from "./thin-macos-libmpv.mjs";

let failures = 0;
function assert(cond, label, detail) {
  if (cond) {
    process.stdout.write(`ok   ${label}\n`);
  } else {
    process.stderr.write(`FAIL ${label}\n`);
    if (detail) process.stderr.write(`     ${detail}\n`);
    failures += 1;
  }
}

function equal(actual, expected) {
  return JSON.stringify(actual) === JSON.stringify(expected);
}

// --- parseLipoArchs: `lipo -archs` prints one line, space-separated ---------
{
  assert(
    equal(parseLipoArchs("arm64\n"), ["arm64"]),
    "a thin arm64 file parses to one architecture",
  );
  assert(
    equal(parseLipoArchs("x86_64 arm64\n"), ["arm64", "x86_64"]),
    "a universal file parses to both architectures, sorted",
  );
  assert(
    equal(parseLipoArchs("  arm64   x86_64  arm64 \n"), ["arm64", "x86_64"]),
    "duplicate and padded entries are normalised",
  );
  let threw = false;
  try {
    parseLipoArchs(" \n");
  } catch {
    threw = true;
  }
  assert(threw, "empty lipo output is rejected instead of looking architecture-free");
}

// --- parseInstallNames: a universal binary repeats the name per slice --------
{
  // Verbatim shape of `otool -D` on a fat dylib: one section per architecture,
  // each repeating the same install name. Comparing the raw lines would report
  // drift after lipo -thin even though the install name never changed.
  const FAT = `libavcodec.dylib (architecture x86_64):
@rpath/libavcodec.dylib
libavcodec.dylib (architecture arm64):
@rpath/libavcodec.dylib
`;
  assert(
    equal(parseInstallNames(FAT), ["@rpath/libavcodec.dylib"]),
    "repeated per-architecture install names collapse to one entry",
  );
  assert(
    equal(parseInstallNames("a.dylib:\n@rpath/liba.dylib\n"), ["@rpath/liba.dylib"]),
    "a thin binary's install name is preserved",
  );
  assert(
    equal(
      parseInstallNames("@rpath/z.dylib\n@rpath/a.dylib\n@rpath/z.dylib\n"),
      ["@rpath/a.dylib", "@rpath/z.dylib"],
    ),
    "multiple install names are deduplicated and sorted",
  );
}

// --- classifyArchitectures: exact set equality is what the Gate needs -------
{
  const match = classifyArchitectures(["arm64"], ["arm64"]);
  assert(match.status === "match", "equal sets classify as match", JSON.stringify(match));

  const thin = classifyArchitectures(["arm64", "x86_64"], ["arm64"]);
  assert(
    thin.status === "thin" && equal(thin.extra, ["x86_64"]) && thin.missing.length === 0,
    "a strict superset classifies as thin and names the extra slice",
    JSON.stringify(thin),
  );

  const missing = classifyArchitectures(["arm64"], ["arm64", "x86_64"]);
  assert(
    missing.status === "missing" && equal(missing.missing, ["x86_64"]),
    "a missing declared architecture fails as missing",
    JSON.stringify(missing),
  );

  const swapped = classifyArchitectures(["arm64"], ["x86_64"]);
  assert(
    swapped.status === "missing" && equal(swapped.missing, ["x86_64"]) && equal(swapped.extra, ["arm64"]),
    "a wrong architecture reports both what is missing and what is extra",
    JSON.stringify(swapped),
  );

  const orderInsensitive = classifyArchitectures(["arm64", "x86_64"], ["x86_64", "arm64"]);
  assert(orderInsensitive.status === "match", "set comparison ignores declaration order");
}

// --- planThinning: manifest files are the work list, declaration is expected -
const MANIFEST = {
  architectures: ["arm64"],
  fileArchitectures: {
    "libclean.dylib": ["arm64"],
    "libfat.dylib": ["arm64", "x86_64"],
  },
  files: {
    "libclean.dylib": "clean-sha",
    "libfat.dylib": "fat-sha",
    "libmissing.dylib": "missing-sha",
  },
};

{
  const before = JSON.stringify(MANIFEST);
  const plan = planThinning(MANIFEST, {
    "libclean.dylib": ["arm64"],
    "libfat.dylib": ["arm64", "x86_64"],
    "libmissing.dylib": ["x86_64"],
  });
  assert(
    equal(
      plan.map((entry) => [entry.filename, entry.status]),
      [
        ["libclean.dylib", "match"],
        ["libfat.dylib", "thin"],
        ["libmissing.dylib", "missing"],
      ],
    ),
    "the plan follows manifest.files order and classifies each file",
    JSON.stringify(plan),
  );
  assert(
    JSON.stringify(MANIFEST) === before,
    "planning a dry-run does not mutate the manifest",
  );
}

// --- findHashMismatches: stale pins must fail before any write --------------
{
  const mismatches = findHashMismatches(MANIFEST, {
    "libclean.dylib": "clean-sha",
    "libfat.dylib": "not-fat-sha",
    "libmissing.dylib": "missing-sha",
  });
  assert(
    equal(mismatches.map((entry) => entry.filename), ["libfat.dylib"]),
    "only files whose bytes diverge from the manifest are reported",
    JSON.stringify(mismatches),
  );
  const absent = findHashMismatches(MANIFEST, {
    "libclean.dylib": "clean-sha",
    "libfat.dylib": "fat-sha",
  });
  assert(
    equal(absent.map((entry) => entry.filename), ["libmissing.dylib"]),
    "a file with no computed hash fails closed rather than passing",
    JSON.stringify(absent),
  );
}

// --- applyVendorState: rewrite architecture and hash without mutating input -
{
  const before = JSON.stringify(MANIFEST);
  const next = applyVendorState(MANIFEST, [
    { filename: "libfat.dylib", architectures: ["arm64"], sha256: "thinned-sha" },
  ]);
  assert(
    equal(next.fileArchitectures["libfat.dylib"], ["arm64"]),
    "fileArchitectures records the post-thinning architecture set",
    JSON.stringify(next.fileArchitectures),
  );
  assert(
    next.files["libfat.dylib"] === "thinned-sha" &&
      next.files["libclean.dylib"] === "clean-sha",
    "only the rewritten file's hash changes",
  );
  assert(JSON.stringify(MANIFEST) === before, "the input manifest is not mutated");
  assert(
    equal(Object.keys(next.files), Object.keys(MANIFEST.files)),
    "file order and membership are preserved for a stable diff",
  );

  let threw = false;
  try {
    applyVendorState(MANIFEST, [
      { filename: "libunknown.dylib", architectures: ["arm64"], sha256: "x" },
    ]);
  } catch {
    threw = true;
  }
  assert(threw, "a state entry for an unknown manifest file is rejected");
}

// --- isPackagedDylibReference: packaged links resolve inside the bundle -----
{
  // The arm64 slice links FFmpeg through @loader_path while the old x86_64
  // slice used @rpath. Both resolve to the flat Contents/Frameworks directory,
  // so both are packaged; absolute names, subdirectories and traversals escape.
  assert(
    isPackagedDylibReference("@loader_path/libavcodec.dylib"),
    "@loader_path resolves against the dylib's own directory, inside the bundle",
  );
  assert(
    isPackagedDylibReference("@rpath/libavcodec.dylib"),
    "@rpath resolves through the executable's bundled Frameworks rpath",
  );
  assert(
    !isPackagedDylibReference("/opt/homebrew/opt/ffmpeg/lib/libavcodec.dylib"),
    "an absolute path outside the app bundle is rejected",
  );
  assert(
    !isPackagedDylibReference("@loader_path/../libavcodec.dylib"),
    "a parent traversal out of Frameworks is rejected",
  );
  assert(
    !isPackagedDylibReference("@loader_path/sub/libavcodec.dylib"),
    "a subdirectory is not where bundled dylibs live and is rejected",
  );
  assert(
    !isPackagedDylibReference("libavcodec.dylib"),
    "a bare library name has no packaged resolution rule and is rejected",
  );
  assert(!isPackagedDylibReference(""), "an empty reference is rejected");
}

process.stdout.write(
  failures === 0
    ? "\nthin-macos-libmpv self-test: all ok\n"
    : `\nthin-macos-libmpv self-test: ${failures} failure(s)\n`,
);
process.exit(failures === 0 ? 0 : 1);
