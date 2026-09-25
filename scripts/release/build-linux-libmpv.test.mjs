#!/usr/bin/env node
// Task 2.2 self-test: build-linux-libmpv.mjs must emit the two provenance
// documents the rest of the pipeline already depends on.
//
// Before this, the script produced only the .so files + build.tag, so
//   - release.yml's "Stage platform NOTICE" step failed (no NOTICE.md), and
//   - task-9.7's Linux branch stayed in its "pending, vendor not landed" path
//     because manifest.json never existed.
// Both are load-bearing: task-9.7 verifies per-file SHA-256 and the glibc
// ceiling *through* manifest.json, so the manifest is the single source of
// truth for those checks, not a convenience file.
//
// These tests drive the real manifest/NOTICE writers against a temp vendor
// tree of fake .so blobs. No mpv-build, no readelf, no network.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, lstatSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const digestOf = (b) => createHash("sha256").update(b).digest("hex");

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const SCRIPT = resolve(ROOT, "scripts", "release", "build-linux-libmpv.mjs");

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

// The writers are exercised through a dedicated entry point so the test never
// triggers the mpv-build clone/compile path.
const EMIT = "--emit-provenance";

function emit(vendorDir, buildTag) {
  return spawnSync(
    process.execPath,
    [SCRIPT, EMIT, "--out", vendorDir, "--build-tag", JSON.stringify(buildTag)],
    { cwd: ROOT, encoding: "utf8" },
  );
}

function seedVendor() {
  const dir = mkdtempSync(join(tmpdir(), "echo-libmpv-manifest-"));
  // Names matter: task-9.7's glibc sweep only inspects versioned sonames
  // (.so.2.5 / .so.61 / .so.59 / .so.5 / .so.8 / .so.10), so a fixture that
  // omits them would pass vacuously and prove nothing.
  const blobs = {
    "libmpv.so.2.5": "mpv-client-blob",
    "libavcodec.so.61": "avcodec-blob",
    "libavformat.so.59": "avformat-blob",
    "libavutil.so.59": "avutil-blob",
    "libswresample.so.5": "swresample-blob",
    "libswscale.so.8": "swscale-blob",
    "libavfilter.so.10": "avfilter-blob",
  };
  for (const [name, body] of Object.entries(blobs)) {
    writeFileSync(join(dir, name), body);
  }
  return { dir, names: Object.keys(blobs), blobs };
}

const buildTag = {
  script: "abc123",
  components: {
    mpv: { pin: "aaaaaaa", head: "aaaaaaa" },
    ffmpeg: { pin: "bbbbbbb", head: "bbbbbbb" },
    libplacebo: { pin: "ccccccc", head: "ccccccc" },
    libass: { pin: "ddddddd", head: "ddddddd" },
  },
  libs: 7,
};

// --- 1. manifest.json is written and lists every collected .so -------------
{
  const { dir, names, blobs } = seedVendor();
  const r = emit(dir, buildTag);
  assert(
    r.status === 0,
    "emit-provenance exits 0 on a seeded vendor tree",
    r.stderr,
  );

  let manifest = null;
  try {
    manifest = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
    assert(true, "manifest.json is written and parses as JSON");
  } catch (e) {
    assert(false, "manifest.json is written and parses as JSON", e.message);
  }

  if (manifest) {
    assert(manifest.platform === "linux", "manifest.platform is linux");
    assert(manifest.schemaVersion === 1, "manifest.schemaVersion is 1");

    const listed = Object.keys(manifest.files ?? {}).sort();
    assert(
      JSON.stringify(listed) === JSON.stringify([...names].sort()),
      "manifest.files lists exactly the collected versioned .so set",
      `got ${JSON.stringify(listed)}`,
    );

    // Per-file SHA-256 is what task-9.7 re-verifies on every Linux Gate run;
    // a wrong or missing digest makes the Gate red on a correct build.
    const allMatch = names.every(
      (n) => manifest.files[n] === digestOf(blobs[n]),
    );
    assert(
      allMatch,
      "every manifest.files entry is the real SHA-256 of the vendored blob",
      `expected libmpv.so.2.5=${digestOf(blobs["libmpv.so.2.5"])} got ${manifest.files["libmpv.so.2.5"]}`,
    );

    // The four component pins are how the next build becomes reproducible
    // (loadPins reads exactly these keys and refuses drift).
    assert(
      manifest.mpvCommit === "aaaaaaa" &&
        manifest.ffmpegCommit === "bbbbbbb" &&
        manifest.libplaceboCommit === "ccccccc" &&
        manifest.libassCommit === "ddddddd",
      "manifest records all four component commit pins from build.tag",
      `got ${JSON.stringify({
        mpv: manifest.mpvCommit,
        ffmpeg: manifest.ffmpegCommit,
        libplacebo: manifest.libplaceboCommit,
        libass: manifest.libassCommit,
      })}`,
    );

    assert(
      typeof manifest.glibcMax === "number" && manifest.glibcMax <= 35,
      "manifest records the verified glibc ceiling at or below 2.35",
      `got ${JSON.stringify(manifest.glibcMax)}`,
    );
  }

  // --- 2. NOTICE.md exists and names the self-built provenance -------------
  let notice = "";
  try {
    notice = readFileSync(join(dir, "NOTICE.md"), "utf8");
    assert(true, "NOTICE.md is written into the vendor dir");
  } catch (e) {
    assert(false, "NOTICE.md is written into the vendor dir", e.message);
  }

  if (notice) {
    // release.yml only checks existence, but the file's whole job is to be an
    // honest provenance record for a *self-built* (not downloaded) runtime.
    assert(
      notice.includes("LGPL-2.1-or-later"),
      "NOTICE names the LGPL obligation of libmpv/FFmpeg",
    );
    assert(
      notice.includes("aaaaaaa") && notice.includes("bbbbbbb"),
      "NOTICE records the built-from commit pins",
    );
  }

  rmSync(dir, { recursive: true, force: true });
}

// --- 3. a first build (no pins) still produces a manifest -------------------
// loadPins returns undefined before task 2.2 lands the manifest; the first CI
// build must still be able to record what it resolved.
{
  const { dir } = seedVendor();
  const unpinned = {
    script: "abc123",
    components: {
      mpv: { pin: "first-build upstream HEAD", head: "1111111" },
      ffmpeg: { pin: "first-build upstream HEAD", head: "2222222" },
      libplacebo: { pin: "first-build upstream HEAD", head: "3333333" },
      libass: { pin: "first-build upstream HEAD", head: "4444444" },
    },
    libs: 7,
  };
  const r = emit(dir, unpinned);
  assert(
    r.status === 0,
    "emit-provenance exits 0 for a first build with no manifest pins",
    r.stderr,
  );
  try {
    const m = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
    assert(
      m.mpvCommit === "1111111" &&
        m.ffmpegCommit === "2222222" &&
        m.libplaceboCommit === "3333333" &&
        m.libassCommit === "4444444",
      "first build records the resolved HEADs so the next build can pin them",
      `got ${JSON.stringify({
        mpv: m.mpvCommit,
        ffmpeg: m.ffmpegCommit,
      })}`,
    );
  } catch (e) {
    assert(false, "first build still writes a parseable manifest.json", e.message);
  }
  rmSync(dir, { recursive: true, force: true });
}

// --- 4. no provenance is written for a tree missing expected libs ----------
{
  const dir = mkdtempSync(join(tmpdir(), "echo-libmpv-manifest-bad-"));
  writeFileSync(join(dir, "libmpv.so.2.5"), "only-mpv");
  const r = emit(dir, buildTag);
  // Guard against passing for the wrong reason: an unrecognised flag also exits
  // non-zero, so require the failure to name the actual missing library.
  assert(
    r.status !== 0 && /libavcodec|libavformat/.test(r.stderr),
    "emit-provenance fails when the vendor tree has no FFmpeg siblings",
    `status=${r.status} stderr=${r.stderr.trim()}`,
  );
  rmSync(dir, { recursive: true, force: true });
}

// --- 5. the libmpv.so symlink is not treated as a shipped file -------------
// collectArtifacts() symlinks libmpv.so → libmpv.so.2.5. statSync follows
// links, so a naive "is it a file?" filter admits the symlink and hashes the
// target under two names. The manifest must carry the real versioned file
// only: task-9.7's glibc sweep and the DT_NEEDED/$ORIGIN resolution both key
// off versioned sonames, and a duplicate digest entry only obscures that.
{
  const dir = mkdtempSync(join(tmpdir(), "echo-libmpv-manifest-link-"));
  const names = [
    "libmpv.so.2.5",
    "libavcodec.so.61",
    "libavformat.so.59",
    "libavutil.so.59",
    "libswresample.so.5",
    "libswscale.so.8",
    "libavfilter.so.10",
  ];
  for (const n of names) writeFileSync(join(dir, n), `body:${n}`);
  symlinkSync("libmpv.so.2.5", join(dir, "libmpv.so"));

  const r = emit(dir, buildTag);
  assert(r.status === 0, "emit-provenance succeeds on a tree with the libmpv.so symlink", r.stderr);
  const m = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
  assert(
    !("libmpv.so" in m.files),
    "manifest.files omits the libmpv.so symlink",
    `got ${JSON.stringify(Object.keys(m.files))}`,
  );
  assert(
    m.files["libmpv.so.2.5"] === digestOf("body:libmpv.so.2.5"),
    "manifest.files records the versioned real file's own digest",
  );
  assert(
    m.libmpvAbi === "2.5",
    "libmpvAbi is read from the versioned soname",
    `got ${JSON.stringify(m.libmpvAbi)}`,
  );
  assert(
    lstatSync(join(dir, "libmpv.so")).isSymbolicLink(),
    "the libmpv.so symlink survives emit-provenance",
  );
  rmSync(dir, { recursive: true, force: true });
}

process.stdout.write(
  failures === 0
    ? "\nbuild-linux-libmpv provenance self-test: all ok\n"
    : `\nbuild-linux-libmpv provenance self-test: ${failures} failure(s)\n`,
);
process.exit(failures === 0 ? 0 : 1);
