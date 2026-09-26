#!/usr/bin/env node
// Self-test for style-dmg.mjs's mounted-image detection.
//
// The macOS release runner failed at `hdiutil convert ... -format UDZO` with
// `Resource temporarily unavailable`. `convert` reads its source image, and
// every detach in style-dmg.mjs is best-effort (the `catch {}` blocks exist
// because Finder's close/reopen transiently unlocks the volume), so "detach
// reported no error" never established "the image is actually released".
//
// The fix is quiesceImage(): treat "still attached" as a hard precondition
// rather than a post-mortem diagnosis. Its load-bearing piece is the parser
// below, so that is what gets covered here — against text trimmed from real
// `hdiutil info` output, since the failure needs a genuinely attached volume
// and a CI runner rarely has one.

import { parseAttachedDevices } from "./style-dmg.mjs";

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

// Trimmed verbatim from `hdiutil info` on macOS: the ==== separator, the
// key/value header for one image, then that image's /dev partition lines.
const TWO_IMAGES = `
framework       : 704
================================================
image-path      : /tmp/echo-dmg-style-abc/rw.dmg
image-alias     : /tmp/echo-dmg-style-abc/rw.dmg
image-type      : read-write disk image
writeable       : true
process ID      : 4711
/dev/disk5	GUID_partition_scheme
/dev/disk5s1	48465300-0000-11AA-AA11-00306543ECAC	/tmp/echo-dmg-mnt-xyz
================================================
image-path      : /Users/xian/Downloads/other.dmg
image-alias     : /Users/xian/Downloads/other.dmg
image-type      : read-only disk image
writeable       : false
process ID      : 47211
/dev/disk4	GUID_partition_scheme
/dev/disk4s1	48465300-0000-11AA-AA11-00306543ECAC	/Volumes/Other
`;

{
  const devices = parseAttachedDevices(TWO_IMAGES);
  assert(
    devices.get("/tmp/echo-dmg-style-abc/rw.dmg") === "/dev/disk5",
    "a mounted image maps to its whole-disk device, not the slice",
    `got ${devices.get("/tmp/echo-dmg-style-abc/rw.dmg")}`,
  );
  // The -mountpoint attach is deliberately not a /Volumes path, so this also
  // pins that we key off image-path rather than off a volume name.
  assert(
    devices.get("/Users/xian/Downloads/other.dmg") === "/dev/disk4",
    "a second image in the same output maps to its own device",
    `got ${devices.get("/Users/xian/Downloads/other.dmg")}`,
  );
  assert(
    devices.size === 2,
    "every attached image is reported, and nothing else is",
    `got ${devices.size} entries: ${JSON.stringify([...devices])}`,
  );
  // /dev/disk5s1 and /dev/disk5s2 are slices of the same image. Keying on the
  // first /dev line per block is what keeps the partition scheme node, which
  // is what `hdiutil detach` wants.
  assert(
    ![...devices.values()].some((d) => /s\d+$/.test(d)),
    "the mapped device is the whole disk, not a partition slice",
    `got ${JSON.stringify([...devices.values()])}`,
  );
}

// --- an image that is not mounted must not be reported as attached ----------
// This is the assertion that makes quiesceImage() return early on the happy
// path. If the parser matched loosely (e.g. on any /dev line without pairing
// it with an image-path), a stale block could make a released image look
// permanently busy and hang the release for the whole settle window.
{
  assert(
    parseAttachedDevices("framework : 704\n").size === 0,
    "an output with no image blocks reports nothing attached",
  );
  assert(
    !parseAttachedDevices(TWO_IMAGES).has("/tmp/echo-dmg-style-abc/styled.dmg"),
    "an image absent from the output is reported as not attached",
  );
}

process.stdout.write(
  failures === 0
    ? "\nstyle-dmg mounted-image detection self-test: all ok\n"
    : `\nstyle-dmg mounted-image detection self-test: ${failures} failure(s)\n`,
);
process.exit(failures === 0 ? 0 : 1);
