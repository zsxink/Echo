#!/usr/bin/env node
// Echo fixture generator.
//
// Reproducibly regenerates every byte of fixtures/audio from scratch, using
// only Node's stdlib plus the system `ffmpeg` binary. All audio is a short
// generated sine wave — no third-party or rights-encumbered media is ever
// committed. Repeated runs must produce byte-identical files.
//
// Determinism notes (verified empirically on ffmpeg 8.1.2 / Homebrew):
//   - mp3 (libmp3lame), m4a (aac), flac, wav: byte-deterministic across runs by
//     default.
//   - mp4/mov muxing (audio+video and video-only fixtures): byte deterministic
//     across runs on the same machine.
//   - ogg/opus and ogg/vorbis: FFmpeg randomizes the Ogg stream serial number
//     on every run (even with `-bitexact` on this build). The audio payload
//     bytes are deterministic; only the container serial (and the page CRC,
//     which covers that serial) varies. gen-fixtures.mjs therefore rewrites
//     every Ogg page: it pins the serial number to a fixed constant, resets
//     the page counter, and recomputes the page CRC with the exact algorithm
//     FFmpeg's Ogg muxer uses. The result is a fully valid, spec-compliant Ogg
//     file whose bytes are identical on every run (validated by ffprobe in the
//     task-1.6 check).
//
// The generated file set is documented in fixtures/audio/MANIFEST.md and its
// SHA-256 fingerprints live in fixtures/audio/checksums.sha256 (kept in sync
// by scripts/verify/checks/task-1.6.mjs).

import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync, rmSync } from "node:fs";
import { crc32 as zlibCrc32, deflateSync } from "node:zlib";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const AUDIO_DIR = resolve(ROOT, "fixtures", "audio");
const LYRICS_DIR = resolve(AUDIO_DIR, "lyrics");

function fail(msg) {
  process.stderr.write(`gen-fixtures: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Deterministic PNG cover art.
//
// Minimal 256x256 8-bit RGB PNG (IHDR/IDAT/IEND) written by hand. The IDAT is
// deflate of the raw scanlines via Node's zlib. No third-party code or media.
// CRC32 via Node's built-in zlib.crc32 (Node >= 20.15; the project requires
// Node >= 20). A stdlib-only fallback is provided for older runtimes.
// ---------------------------------------------------------------------------
let crcTableFallback = null;
function crc32(buf) {
  const fn = zlibCrc32;
  if (typeof fn === "function") return fn(buf) >>> 0;
  if (!crcTableFallback) {
    const tbl = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      tbl[n] = c >>> 0;
    }
    crcTableFallback = tbl;
  }
  let crc = 0xffffffff;
  for (const b of buf) crc = crcTableFallback[(crc ^ b) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
  return Buffer.concat([len, typeBuf, data, crc]);
}

// 256x256 RGB gradient + checkerboard accent. Byte-for-byte deterministic.
function makeCoverPng(size = 256) {
  const w = size;
  const h = size;
  const bpp = 3;
  const raw = Buffer.alloc(h * (1 + w * bpp)); // filter byte + RGB per row
  for (let y = 0; y < h; y++) {
    const rowStart = y * (1 + w * bpp);
    raw[rowStart] = 0; // filter type none
    for (let x = 0; x < w; x++) {
      const r = Math.round((x * 255) / (w - 1));
      const g = Math.round((y * 255) / (h - 1));
      const b = 64 + ((x + y) % 2) * 128;
      const off = rowStart + 1 + x * bpp;
      raw[off] = r;
      raw[off + 1] = g;
      raw[off + 2] = b;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type: RGB
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace
  const idat = deflateSync(raw);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", idat),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---------------------------------------------------------------------------
// Deterministic Ogg page re-stamping.
//
// FFmpeg's Ogg muxer randomizes the stream serial number on every run, which
// also changes each page's CRC. The page payloads (all packets) are otherwise
// byte-identical between runs, so we rewrite every page header: fixed serial
// number, sequential page counter (starting at 0), zeroed CRC, then recompute
// the CRC exactly as FFmpeg's av_crc/oggenc does. The result is valid per the
// Ogg spec and identical on every run.
// ---------------------------------------------------------------------------
function bswap32(x) {
  x >>>= 0;
  return (
    (((x >>> 24) & 0xff) |
      ((x >>> 8) & 0xff00) |
      ((x << 8) & 0xff0000) |
      ((x << 24) & 0xff000000)) >>>
    0
  );
}

let crcTableMsb = null;
function buildCrcTableMsb() {
  if (crcTableMsb) return crcTableMsb;
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = (i << 24) >>> 0;
    for (let j = 0; j < 8; j++) {
      c = ((c << 1) ^ (c & 0x80000000 ? 0x04c11db7 : 0)) >>> 0;
    }
    table[i] = c;
  }
  crcTableMsb = table;
  return table;
}

// Standard MSB CRC-32 (non-reflected, poly 0x04c11db7, no final xor), in the
// streaming form FFmpeg's av_crc uses: continue from an existing crc so header
// and body are covered in one pass.
function crc32Msb(crc, buf) {
  crc >>>= 0;
  const table = buildCrcTableMsb();
  for (const b of buf) {
    crc = ((crc << 8) ^ table[((crc >>> 24) ^ b) & 0xff]) >>> 0;
  }
  return crc;
}

// The 4 CRC bytes FFmpeg writes for an Ogg page whose crc field (22..25) is
// zeroed. FFmpeg stores bswap32(standard_msb_crc) as the big-endian 4 bytes.
function oggPageCrcBytes(page) {
  const ns = page[26];
  const header = Buffer.concat([
    page.subarray(0, 22),
    Buffer.from([0, 0, 0, 0]),
    Buffer.from([ns]),
    page.subarray(27, 27 + ns),
  ]);
  const body = page.subarray(27 + ns);
  const crc = crc32Msb(crc32Msb(0, header), body);
  const out = Buffer.alloc(4);
  out.writeUInt32BE(bswap32(crc) >>> 0, 0);
  return out;
}

// Rewrite all Ogg pages of `inputPath`: pin serial to `serialOffset` (every
// fixture here is single-stream), renumber pages from 0, recompute CRCs.
function stampOgg(inputPath, outputPath, serialOffset = 0) {
  const data = readFileSync(inputPath);
  const out = Buffer.from(data);
  let off = 0;
  let pageNum = 0;
  while (off < out.length) {
    if (out.toString("ascii", off, off + 4) !== "OggS") {
      fail(`stampOgg: bad page signature at offset ${off} in ${inputPath}`);
    }
    const ns = out[off + 26];
    const segs = [...out.subarray(off + 27, off + 27 + ns)];
    const bodyLen = segs.reduce((a, b) => a + b, 0);
    const pageEnd = off + 27 + ns + bodyLen;
    out.writeUInt32LE(serialOffset >>> 0, off + 14);
    out.writeUInt32LE(pageNum >>> 0, off + 18);
    out.writeUInt32LE(0, off + 22);
    const crcBytes = oggPageCrcBytes(out.subarray(off, pageEnd));
    crcBytes.copy(out, off + 22);
    pageNum++;
    off = pageEnd;
  }
  writeFileSync(outputPath, out);
}

// ---------------------------------------------------------------------------
// ffmpeg invocation helper
// ---------------------------------------------------------------------------
function ffmpeg(args) {
  const r = spawnSync("ffmpeg", ["-y", "-hide_banner", "-loglevel", "error", ...args], {
    encoding: "utf8",
  });
  if (r.status !== 0) {
    fail(
      `ffmpeg ${args.join(" ")}\n    exited ${r.status}\n    ${(r.stderr || "").trim()}`,
    );
  }
}

// ---------------------------------------------------------------------------
// Fixture definitions
// ---------------------------------------------------------------------------
// Shared fixed parameters. Every audio fixture is a 1s 440 Hz sine at 44100 Hz
// mono, with fixed tags, so ffmpeg's output is reproducible.
const SINE = "sine=frequency=440:sample_rate=44100:duration=1";
function meta(title = "Echo Tone", artist = "Echo Fixtures", album = "Fixture Album", date = "2026") {
  return [
    "-metadata", `title=${title}`,
    "-metadata", `artist=${artist}`,
    "-metadata", `album=${album}`,
    "-metadata", `date=${date}`,
  ];
}

function generate() {
  mkdirSync(AUDIO_DIR, { recursive: true });
  mkdirSync(LYRICS_DIR, { recursive: true });

  // A) cover art (not ffmpeg-dependent)
  writeFileSync(resolve(AUDIO_DIR, "cover-256.png"), makeCoverPng(256));

  // B) tone-short.mp3 — ~1s sine, ID3v2 tags via ffmpeg's libmp3lame + Xing.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-codec:a", "libmp3lame", "-b:a", "128k", "-write_xing", "1",
    ...meta(),
    resolve(AUDIO_DIR, "tone-short.mp3"),
  ]);

  // C) tone-short.flac — Vorbis tags + embedded cover picture.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-i", resolve(AUDIO_DIR, "cover-256.png"),
    "-map", "0:a", "-map", "1:v",
    "-c:a", "flac", "-c:v", "png", "-disposition:v", "attached_pic",
    ...meta("Flac Tone"),
    resolve(AUDIO_DIR, "tone-short.flac"),
  ]);

  // D) tone-short.m4a — AAC in MP4 container with title/artist.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-c:a", "aac", "-b:a", "128k",
    ...meta("M4A Tone"),
    resolve(AUDIO_DIR, "tone-short.m4a"),
  ]);

  // E) tone-short.ogg — Vorbis in Ogg. The Homebrew ffmpeg build has no
  // libvorbis; the native FFmpeg vorbis encoder is used (`-strict -2`, stereo
  // only). The resulting Ogg stream is deterministically re-stamped.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-ac", "2",
    "-c:a", "vorbis", "-strict", "-2", "-q:a", "4",
    ...meta("Ogg Tone"),
    "-f", "ogg",
    resolve(AUDIO_DIR, ".tone-short.ogg.tmp"),
  ]);
  stampOgg(
    resolve(AUDIO_DIR, ".tone-short.ogg.tmp"),
    resolve(AUDIO_DIR, "tone-short.ogg"),
    1,
  );
  rmSync(resolve(AUDIO_DIR, ".tone-short.ogg.tmp"), { force: true });

  // F) tone-short.opus — Opus in Ogg, deterministically re-stamped.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-c:a", "libopus", "-b:a", "96k", "-application", "audio",
    ...meta("Opus Tone"),
    "-f", "ogg",
    resolve(AUDIO_DIR, ".tone-short.opus.tmp"),
  ]);
  stampOgg(
    resolve(AUDIO_DIR, ".tone-short.opus.tmp"),
    resolve(AUDIO_DIR, "tone-short.opus"),
    2,
  );
  rmSync(resolve(AUDIO_DIR, ".tone-short.opus.tmp"), { force: true });

  // G) tone-short.wav — PCM s16le WAV. Tag support in WAV is limited; fine.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-c:a", "pcm_s16le",
    resolve(AUDIO_DIR, "tone-short.wav"),
  ]);

  // H) tone-short-video.mp4 — MP4 with an audio track (sine) + tiny video.
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-f", "lavfi", "-i", "color=c=black:s=64x64:r=1:d=1",
    "-map", "0:a", "-map", "1:v",
    "-c:a", "aac", "-b:a", "96k",
    "-c:v", "libx264", "-t", "1", "-pix_fmt", "yuv420p", "-shortest",
    ...meta("Tone Video"),
    resolve(AUDIO_DIR, "tone-short-video.mp4"),
  ]);

  // I) no-audio.mp4 — MP4 with no audio track (video only).
  ffmpeg([
    "-f", "lavfi", "-i", "color=c=steelblue:s=64x64:r=1:d=1",
    "-c:v", "libx264", "-t", "1", "-pix_fmt", "yuv420p", "-an",
    resolve(AUDIO_DIR, "no-audio.mp4"),
  ]);

  // J) tone-corrupted.mp3 — deliberately corrupted container. Generate the
  // full mp3, then truncate mid-frame: keep the ID3v2 tag plus the first few
  // bytes of the first MPEG audio frame, so no complete frame survives and a
  // media probe reports the file as invalid. (A 60% whole-file truncation was
  // evaluated first, but MP3's streaming resilience means ffprobe still
  // accepts it; truncating inside the first frame produces a genuinely broken
  // container while keeping a plausible `.mp3` extension.)
  ffmpeg([
    "-f", "lavfi", "-i", SINE,
    "-codec:a", "libmp3lame", "-b:a", "128k", "-write_xing", "1",
    ...meta(),
    "-f", "mp3",
    resolve(AUDIO_DIR, ".tone-full.mp3.tmp"),
  ]);
  {
    const full = readFileSync(resolve(AUDIO_DIR, ".tone-full.mp3.tmp"));
    let sync = -1;
    for (let p = Math.min(10, full.length); p < full.length - 1; p++) {
      // 0xff followed by a byte whose top 3 bits are 111 (MPEG audio frame sync)
      if (full[p] === 0xff && (full[p + 1] & 0xe0) === 0xe0) {
        sync = p;
        break;
      }
    }
    if (sync < 0) fail("could not locate first MPEG frame sync in generated mp3");
    // Keep the ID3 tag + 3 bytes of the first frame -> broken mid-frame.
    writeFileSync(resolve(AUDIO_DIR, "tone-corrupted.mp3"), full.subarray(0, sync + 3));
  }
  rmSync(resolve(AUDIO_DIR, ".tone-full.mp3.tmp"), { force: true });

  // K) Lyrics.
  // K1) tone-short.lrc — synced LRC sidecar for tone-short.mp3 (Echo's sidecar
  // convention: <basename>.lrc next to the audio).
  const synced = [
    "[ti:Echo Tone]",
    "[ar:Echo Fixtures]",
    "[al:Fixture Album]",
    "[00:00.00]Starting the tone",
    "[00:00.25]Quarter through the sine",
    "[00:00.50]Middle of the fixture tone",
    "[00:00.75]Nearly done",
    "[00:01.00]Tone complete",
    "",
  ].join("\n");
  writeFileSync(resolve(AUDIO_DIR, "tone-short.lrc"), synced);

  // K2) lyrics/tone-short.synced.lrc — same synced lyrics kept in the lyrics
  // subdir for consumers that look there.
  writeFileSync(resolve(LYRICS_DIR, "tone-short.synced.lrc"), synced);

  // K3) lyrics/tone-short.plain.txt — plain-text lyrics, no timestamps.
  const plain = [
    "Echo Fixtures — Tone Lyrics",
    "",
    "Starting the tone",
    "Quarter through the sine",
    "Middle of the fixture tone",
    "Nearly done",
    "Tone complete",
    "",
  ].join("\n");
  writeFileSync(resolve(LYRICS_DIR, "tone-short.plain.txt"), plain);
}

generate();
console.log("gen-fixtures: regenerated fixtures under fixtures/audio/");