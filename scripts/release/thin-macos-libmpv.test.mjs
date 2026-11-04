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
  compareVendorSets,
  computeVendorClosure,
  countLocalSymbols,
  findHashMismatches,
  isPackagedDylibReference,
  parseDylibReferences,
  parseInstallNames,
  parseLipoArchs,
  planThinning,
  planVendorSteps,
  vendorSetsMatch,
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

// --- planVendorSteps: strip always precedes re-signing ----------------------
{
  const clean = planVendorSteps({
    filename: "libmpv.dylib",
    status: "match",
    expected: ["arm64"],
  });
  assert(
    equal(clean.map((step) => step.command), [
      "chmod",
      "otool",
      "strip",
      "codesign",
    ]),
    "an already-arm64 file is still stripped and re-signed",
    JSON.stringify(clean),
  );
  const stripStep = clean.find((step) => step.command === "strip");
  const signStep = clean.find((step) => step.command === "codesign");
  assert(
    equal(stripStep.args, ["-x"]) &&
      clean.indexOf(stripStep) < clean.indexOf(signStep),
    "strip -x runs before the ad-hoc re-sign (it invalidates signatures)",
    JSON.stringify(clean),
  );

  const fat = planVendorSteps({
    filename: "libfat.dylib",
    status: "thin",
    expected: ["arm64"],
  });
  assert(
    equal(fat.map((step) => step.command), [
      "lipo",
      "chmod",
      "otool",
      "strip",
      "codesign",
    ]),
    "a fat file is thinned, then stripped, then re-signed in that order",
    JSON.stringify(fat),
  );
  assert(
    equal(fat[0].args, ["-thin", "arm64"]),
    "the lipo step targets the single release architecture",
    JSON.stringify(fat[0]),
  );

  let threw = false;
  try {
    planVendorSteps({ filename: "libbad.dylib", status: "missing", expected: ["arm64"] });
  } catch {
    threw = true;
  }
  assert(threw, "a file missing its declared architecture is never repaired in place");
}

// --- countLocalSymbols: lowercase locals only, uppercase externals survive ---
{
  const UNSTRIPPED = `0000000000001234 t _local_text
0000000000001238 s _local_data
0000000000001240 T _exported_text
                 U _dyld_stub_binder
0000000000001250 d _local_bss
`;
  assert(
    countLocalSymbols(UNSTRIPPED) === 3,
    "only lowercase t/s/b/d tokens are counted as local symbols",
    `got ${countLocalSymbols(UNSTRIPPED)}`,
  );
  assert(
    countLocalSymbols("0000000000001234 T _exported\n                 U _malloc\n") === 0,
    "external and undefined symbols are not local symbols",
  );
  assert(countLocalSymbols("") === 0, "empty nm output counts zero local symbols");
}

// --- Gate 9.7 pure helpers: the guard must actually reject violations -------
{
  // Verbatim shape of `otool -L`: the inspected path on line 1, then one
  // reference per line. System frameworks and absolute paths are not packaged.
  const OTOOL = `libmpv.dylib:
\t@loader_path/libavcodec.dylib (compatibility version 60.0.0, current version 60.3.100)
\t/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation (compatibility version 1.0.0, current version 5027.0.69)
\t@rpath/libavutil.dylib (compatibility version 58.0.0, current version 58.2.100)
`;
  assert(
    equal(parseDylibReferences(OTOOL), ["libavcodec.dylib", "libavutil.dylib"]),
    "only flat @loader_path/@rpath dylib references enter the closure",
    JSON.stringify(parseDylibReferences(OTOOL)),
  );

  const references = {
    "libmpv.dylib": ["libavcodec.dylib", "libavutil.dylib"],
    "libavcodec.dylib": ["libavutil.dylib"],
    "libavutil.dylib": [],
    "libunused.dylib": [],
  };
  assert(
    equal(computeVendorClosure("libmpv.dylib", references), [
      "libavcodec.dylib",
      "libavutil.dylib",
      "libmpv.dylib",
    ]),
    "the closure stops at the reachable libraries and excludes unused ones",
    JSON.stringify(computeVendorClosure("libmpv.dylib", references)),
  );

  const clean = compareVendorSets({
    manifestFiles: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib"],
    closure: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib"],
    bundleFrameworks: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib"],
  });
  assert(vendorSetsMatch(clean), "an untouched tree passes the set comparison");

  // Injection: a library is present in manifest and bundle but not referenced
  // by any dylib (the exact mbedTLS regression this change removes).
  const unused = compareVendorSets({
    manifestFiles: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib", "libmbedtls.dylib"],
    closure: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib"],
    bundleFrameworks: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib", "libmbedtls.dylib"],
  });
  assert(
    !vendorSetsMatch(unused) && equal(unused.unusedInManifest, ["libmbedtls.dylib"]),
    "an unreferenced vendored library is rejected and named",
    JSON.stringify(unused),
  );

  // Injection: a manifest/bundle entry is stale after a rebuild.
  const drifted = compareVendorSets({
    manifestFiles: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib"],
    closure: ["libmpv.dylib", "libavcodec.dylib", "libavutil.dylib"],
    bundleFrameworks: ["libmpv.dylib", "libavcodec.dylib"],
  });
  assert(
    !vendorSetsMatch(drifted) && equal(drifted.missingFromBundle, ["libavutil.dylib"]),
    "a missing bundle mapping is rejected and named",
    JSON.stringify(drifted),
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

// --- --write re-pins every manifest file, not only thinned ones -------------
{
  // `strip -x` is a no-op on already-stripped dylibs, but --write still has to
  // refresh every hash: dropping a clean file here would leave its manifest
  // SHA-256 pointing at pre-strip bytes on the next rebuild.
  const all = Object.keys(MANIFEST.files).map((filename) => ({
    filename,
    architectures: ["arm64"],
    sha256: `strip-sha-${filename}`,
  }));
  const next = applyVendorState(MANIFEST, all);
  assert(
    Object.entries(next.files).every(([name, hash]) => hash === `strip-sha-${name}`),
    "all manifest files receive a refreshed SHA-256 after a strip run",
    JSON.stringify(next.files),
  );
  assert(
    Object.keys(next.fileArchitectures).length === 3,
    "fileArchitectures is rewritten for every file, including clean ones",
    JSON.stringify(next.fileArchitectures),
  );
  assert(
    equal(Object.keys(next.files), Object.keys(MANIFEST.files)),
    "a full re-pin does not add or remove manifest entries",
  );
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
