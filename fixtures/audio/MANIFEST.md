# fixtures/audio — licensed-clear generated fixture manifest

All media in this directory is **generated** by `scripts/gen-fixtures.mjs` (or
derived from its output): short sine/silence tones, a programmatic cover PNG,
and hand-written lyric text files. There is **no third-party or
rights-encumbered content** here — no ripped, downloaded, sampled, or
copyrighted audio, images, or lyrics. Everything is reproducible from the
generator with the system `ffmpeg` binary and Node's stdlib.

Regeneration command:

```sh
node scripts/gen-fixtures.mjs
```

Checksum verification (from this directory):

```sh
shasum -a 256 -c checksums.sha256
# or, portably:
sha256sum -c checksums.sha256
```

`scripts/verify/checks/task-1.6.mjs` re-runs the generator, recomputes the
SHA-256 of every committed fixture in Node, and asserts that the manifest below
documents every file actually present in this tree.

## Determinism note (Ogg)

`mp3`, `m4a`, `flac`, `wav`, and both `mp4` fixtures are byte-deterministic
across runs on a given machine. FFmpeg's Ogg muxer randomizes the Ogg stream
serial number on every run (and on ffmpeg 8.1.x even with `-bitexact`), which
changes the per-page CRC. The Ogg *payload* bytes are deterministic; the
generator therefore re-stamps every Ogg page — fixed serial, sequential page
counters, recomputed CRCs using FFmpeg's own algorithm — producing fully valid,
byte-identical Ogg / Opus files on every run (verified by `ffprobe`, and by the
determinism check in `task-1.6.mjs`).

## Files

| File | Size | Format | Covers | Source method |
|---|---|---|---|---|
| `cover-256.png` | 143 K | PNG, 256×256 RGB | cover art (embedded in FLAC) | Hand-written minimal PNG (IHDR/IDAT/IEND) rendered in Node: 256×256 gradient + checkerboard, deflate via Node `zlib`. No third-party code or media. |
| `tone-short.mp3` | 17 K | MP3 (ID3v2 + Xing) | guaranteed `.mp3`, ID3 tags | `ffmpeg -f lavfi -i sine=frequency=440:sample_rate=44100:duration=1 -codec:a libmp3lame -b:a 128k -write_xing 1` + fixed `title/artist/album/date` metadata |
| `tone-short.flac` | 20 K | FLAC | guaranteed `.flac`, Vorbis tags, embedded cover | `ffmpeg -f lavfi -i <sine> -i cover-256.png -map 0:a -map 1:v -c:a flac -c:v png -disposition:v attached_pic` + fixed metadata |
| `tone-short.m4a` | 15 K | M4A/AAC | guaranteed `.m4a`, tags | `ffmpeg -f lavfi -i <sine> -c:a aac -b:a 128k` + fixed `title/artist` metadata |
| `tone-short.ogg` | 6.9 K | Ogg/Vorbis | guaranteed `.ogg`, tags | `ffmpeg -f lavfi -i <sine> -ac 2 -c:a vorbis -strict -2 -q:a 4` + fixed metadata (native FFmpeg vorbis encoder; this Homebrew build has no libvorbis), then deterministic Ogg re-stamp |
| `tone-short.opus` | 15 K | Ogg/Opus | guaranteed `.opus`, tags | `ffmpeg -f lavfi -i <sine> -c:a libopus -b:a 96k -application audio` + fixed metadata, then deterministic Ogg re-stamp |
| `tone-short.wav` | 86 K | WAV/PCM s16le | guaranteed `.wav` (WAV has no real tag support — documented limitation) | `ffmpeg -f lavfi -i <sine> -c:a pcm_s16le` |
| `tone-short-video.mp4` | 14.6 K | MP4 (AAC + H.264) | "mp4 **with** an audio track" | `ffmpeg -f lavfi -i <sine> -f lavfi -i color=c=black:s=64x64:r=1:d=1 -map 0:a -map 1:v -c:a aac -b:a 96k -c:v libx264 -t 1 -pix_fmt yuv420p -shortest` + fixed metadata |
| `no-audio.mp4` | 1.5 K | MP4 (H.264 only) | corner case: mp4 with **no** audio track | `ffmpeg -f lavfi -i color=c=steelblue:s=64x64:r=1:d=1 -c:v libx264 -t 1 -pix_fmt yuv420p -an` |
| `tone-corrupted.mp3` | 135 B | broken MP3 container | corrupt-media diagnostics | Generate the full `tone-short`-equivalent mp3, then truncate **mid-frame**: keep the ID3v2 tag plus the first 3 bytes of the first MPEG audio frame, so no complete frame survives. `ffprobe` reports `Failed to find two consecutive MPEG audio frames / Invalid data`. (A 60% whole-file truncation was evaluated; MP3's streaming resilience still lets ffprobe open it, so the fixture truncates inside the first frame instead for a genuinely broken container.) |
| `lyrics/tone-short.synced.lrc` | 199 B | LRC (synced) | synced lyrics with `[ti:]` header and `[mm:ss.xx]` lines at 0.00/0.25/0.50/0.75/1.00 | Hand-written (generated) text in `scripts/gen-fixtures.mjs` |
| `tone-short.lrc` | 199 B | LRC (synced) | sidecar pairing (`tone-short.mp3` + `tone-short.lrc`), Echo's `<basename>.lrc` convention | Same synced LRC content as above, placed next to the mp3 |
| `lyrics/tone-short.plain.txt` | 127 B | plain text | plain-text lyrics candidate, no timestamps | Hand-written (generated) text in `scripts/gen-fixtures.mjs` |

Every audio fixture is a 1-second 440 Hz sine at 44.1 kHz, mono (Ogg/Vorbis is
stereo because the native vorbis encoder requires 2 channels), fixed bitrates,
fixed metadata (`title=Echo Tone`, `artist=Echo Fixtures`, `album=Fixture
Album`, `date=2026` variants). All generated content is original and
license-clear: zero third-party rights attach to any file in this directory.

## Licensing statement

- Audio: generated sine tones — original, public-domain-equivalent, no third-party rights.
- Cover art: programmatically generated gradient/checkerboard — original, no rights issues.
- Lyrics: original text generated for the fixture tones — no third-party lyrics reproduced.
- No file in this directory is a copy or rip of any commercial recording.

This file, `checksums.sha256`, and the generator at `scripts/gen-fixtures.mjs`
together guarantee the "repo contains only authorized/licensed media" invariant:
a check re-runs the deterministic generator and fails loudly if the committed
bytes no longer match, or if any file present below is not documented here.