# Audio effects native Gate tools

These tools prepare experiments for `introduce-audio-effects-equalizer`, task group 1.
They do **not** complete the three-platform Gate or implement production effects.
Only Python 3.9+ standard-library modules are required. Do not commit generated WAVs.

## What counts as evidence

`af-command` returning `rc=0` is **not** evidence that a parameter changed.
mpv's `f_lavfi.c` only tests `result >= 0`, so a rejected command is reported as
success, and the `af` property shows only the *configured* description, which
mpv does not rewrite when a runtime command changes a parameter. The only
admissible proof is a PCM measurement across the change.

Rechecked on the packaged macOS library (2026-10-08): runtime EQ, volume and
spatial-width commands **do change PCM**. The old negative result was invalid:
PCM AO decoded the short fixture to EOF before the command was issued. The
corrected probe extends fixture bytes offline before one continuous playback,
records command-time file offsets, and compares baseline and post-command sample
windows. It never pauses, seeks, reloads, or replaces the graph across a change.
Measured changes were +5.9886 dB for EQ 0→+6 dB, −6.0382 dB for preamp 0→−6 dB,
and −1.9474 dB for width 1.25→1 with its preamp held fixed (expected −1.9382 dB).
This proves those commands on this package; it does not establish transition
smoothness, real-time latency, or other-platform behavior.

Because of that, do not treat these as interchangeable:

| Question | Tool | What it proves |
|---|---|---|
| Does a filter expose a command callback? | `inspect-filter-runtime.py` | Structure only, and it refuses output if its layout cross-check fails |
| Did a command change the audio? | `probe.py --runtime-parameter-proof` | Actual EQ/preamp/width change via uninterrupted PCM before/after |
| Is the measurement itself trustworthy? | `probe.py` `static_gain_reference` | The harness detects a known install-time gain |
| Does the chain match the design tolerance? | `probe.py --sweep-response` | **Not yet** — reports `usable_for_tolerance_verdict: false` |

To repeat the silent runtime parameter measurement, generate the fixtures above,
then run:

```sh
python3 scripts/audio-effects/probe.py \
  --libmpv apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib \
  --input /tmp/echo-audio-effects-fixtures/tone997-48000-2ch-60db.wav \
  --fixture-manifest /tmp/echo-audio-effects-fixtures/manifest.json \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --output /tmp/echo-runtime-recheck.json \
  --ao pcm --pcm-output /tmp/echo-runtime-static.wav --capture-only \
  --runtime-parameter-proof --runtime-capture-seconds 1 \
  --step-timeout 10 --timeout 60
```

## mpv `af-command` label semantics (get this wrong and every result is invalid)

This is the single easiest way to produce a confident, wrong conclusion here.

- In `af add` / `af set`, `@label:NAME` splits at the colon: the text between `@`
  and `:` is the **label**; `NAME` is the **filter name**
  (mpv `options/m_option.c`, `@` handling around line 3177).
- `af-command <label> <command> <argument>` resolves the label against mpv's
  user-filter list (`filters/f_output_chain.c`, `find_by_label()`). A label that
  does not exist returns rc = -12.
- **The `af` property reports the filter NAME, not the label.** A chain installed
  as `@echo_eq5:lavfi=[...]` therefore shows up as `lavfi`. That is expected and
  says nothing about whether the label works. Do NOT infer "labels are unusable"
  from this — that mistake invalidated a whole round of measurements here.
- `target=all` takes mpv's broadcast branch, which returns true unconditionally.
  Another way to get rc = 0 with no effect.

Only a PCM measurement across the change is admissible evidence.

## Measurement traps

Each of these produced a confidently wrong number during development:

- **PCM AO is untimed.** A 50 ms wall-clock pump can decode an entire short
  fixture. Commands issued afterward cannot change samples already captured.
  The runtime probe pads PCM bytes to a long temporary input before playback,
  requires baseline PCM and `eof-reached=false` before issuing the command, and
  checks for a sufficiently long post-command sample window. Generated long
  inputs and captures are temporary, not committed fixture-matrix additions.
- **File offsets are not latency measurements.** WAVE buffering and native
  pipeline queues make file byte counts conservative bounds on samples already
  written. The probe analyses distant sample windows and does not use these
  offsets to claim real-time smoothing or audible command latency.
- **Bit depth differs**: fixtures are PCM32, `ao=pcm` emits PCM16. Normalize per
  width, or the format change reads as about −96 dB of response error.
- **Limiter saturation hides changes.** Use a low-level fixture at the input's
  own rate, otherwise both states clip to the same ceiling.

## Generate offline inputs

From the repository root (PowerShell can use an equivalent temporary directory):

```sh
python3 scripts/audio-effects/generate_fixtures.py \
  --output /tmp/echo-audio-effects-fixtures --duration 2
```

The default matrix has 80 signed PCM32 little-endian WAVs:

- 22.05, 44.1, 48 and 96 kHz; mono and stereo.
- A 997 Hz tone, logarithmic 20 Hz–min(20 kHz, 0.45 Fs) sweep, fixed-seed
  xorshift32 broadband noise and a single impulse at one quarter of the duration.
- Stereo-only left-channel tone and opposite-polarity 997 Hz tone. These have no
  mono counterpart because that would erase the intended channel relationship.
- Each signal has a normalized sample peak at -0.1 dBFS and -60 dBFS. PCM32
  quantization is reported separately; -60 dBFS inputs are intended for response
  measurements with the limiter inactive. Tone/noise peak normalization does not
  assert a true-peak ceiling or perceptual loudness.

`manifest.json` records every SHA-256, frames, duration, rate, channels, generator
hash, requested/measured sample peak, signal source and CC0-1.0 license. Echo's
synthetic fixture output is dedicated under [CC0-1.0](https://creativecommons.org/publicdomain/zero/1.0/).
No third-party listening recordings are included. Repeated runs on the same
Python/runtime produce identical bytes; platform libm rounding may differ.
Cross-platform measurements must compare manifest hashes and identify the actual
input bytes. This qualification applies to trigonometric signals, not the pinned
integer noise generator.

## Probe the actual distributed library

Select the exact installed candidate's library; no `find_library`, executable
from PATH, system libmpv or automatic missing-dependency substitution is used.
On POSIX the tool calls native `dlopen` with `RTLD_LAZY | RTLD_LOCAL`, then wraps
the same handle with ctypes. This matches Rust libloading; ctypes `CDLL(path)`
alone forces eager resolution even if given `RTLD_LAZY`. The default is `ao=null`, which discards PCM. On macOS, `--ao coreaudio` can verify real CoreAudio negotiation and time-sensitive runtime commands while playing the synthetic -60 dBFS fixture at 1% mpv volume. `--ao pcm --capture-only` captures an isolated, static EQ chain before CoreAudio and measures integer PCM peak plus 997 Hz response; PCM AO is non-real-time; `--runtime-parameter-proof` uses sample windows from a continuous capture to verify runtime commands without sending sound to hardware. It does not measure device latency or underruns. The tool rejects a missing library instead of choosing a fallback. Run this explicit macOS example from the repository root:

```sh
python3 scripts/audio-effects/probe.py \
  --libmpv /Applications/Echo.app/Contents/Frameworks/libmpv.dylib \
  --input /tmp/echo-audio-effects-fixtures/tone997-48000-2ch-60db.wav \
  --fixture-manifest /tmp/echo-audio-effects-fixtures/manifest.json \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --output /tmp/echo-audio-effects-macos-probe.json --timeout 30 --ao coreaudio
```

On Windows pass the installed package's absolute `libmpv-2.dll` path and its
matching distribution manifest. On Linux pass the installed candidate's absolute
`.so` path; do not infer a distributed library from a system executable. Those
platforms are not validated by the macOS result. `--package <installer-or-archive>`
optionally adds the package SHA-256; an app directory is not an archive hash.
`--step-timeout` bounds waiting for negotiated audio properties, and the supervising
process kills a stalled native worker at `--timeout` (including initialization or
teardown). Failure/timeout exit with status 1 and still write a JSON report.

The report includes the selected library/input hashes, optional package and
manifest hashes, neighboring distribution files compared against that manifest,
client API version, mpv/FFmpeg version properties, available FFmpeg version
exports and `avfilter_get_by_name` availability checks, loaded dependency paths/hashes, every command result/error, native logs
and snapshots of `audio-params`, `audio-out-params`, `af`, `volume` and `mute`.
`load-scripts=no` is optional only when the packaged build reports option-not-found
(-5), as in its audio-only build; all required audio-silencing options must succeed.
Unavailable runtime versions stay null after a failed load; manifest versions are
only package declarations. Runtime dependency mapping uses dyld on macOS,
`/proc/self/maps` on Linux and K32 module enumeration on Windows.

`audio-params` describes decoder output and `audio-out-params` describes the
negotiated AO input. Neither alone proves the sample rate at the `equalizer`
link. The model rate used for the 0.45 Fs band-validity rule and response must
match that link's actual `sample_rate`; source, decoder and AO rates may differ.
Debug logs are retained for inspecting format/rate/layout across filter links and
rebuilds; no source metadata is relabeled as actual DSP processing. Native
properties are retrieved as typed `MPV_FORMAT_NODE`, copied before release.

The experiments install/remove these explicit candidates:

1. A single `@echo_eq:lavfi=[...]` graph: double format negotiation, named
   `volume@preamp`, up to ten `equalizer@eq0`…`eq9` stages and a terminal limiter.
   Each EQ uses `t=q`, `w=1.4142135623730951`, `mix=1`, `normalize=false`,
   `precision=f64`. The FFmpeg 6.0 `equalizer` default block size is retained. Centers are
   31.25/62.5/125/250/500/1000/2000/4000/8000/16000 Hz.
2. Separately labeled outer mpv `@echo_preamp`, `@echo_eq0`…`eq9`,
   `@echo_limiter` lavfi instances as an explicit alternative syntax experiment.
3. `@echo_spatial` with preamp -2.938200260161128 dB and
   `extrastereo=m=1.25:c=false`; the probe refuses mono spatial upmix/downmix.

All candidates use
`alimiter=limit=0.891250938:attack=5:release=50:level_in=1:level_out=1:asc=false:level=false:latency=true`.
Runtime trials read `command-list` from the selected candidate library and record
its `af-command` argument count and the exact client argv, including errors.
The macOS mpv 0.36 compatibility build uses the patched target-prefix syntax:

```text
af-command echo_eq eq5:gain 1
af-command echo_eq preamp:volume 0.501187233627272
```

Upstream mpv 0.37 and newer takes the target as a separate final argument:

```text
af-command echo_eq gain 1 eq5
af-command echo_preamp volume 0.501187233627272 preamp
```

The bundled macOS mpv patch extends `af-command` with `<filter-id>:<option>` for
the named lavfi instance. Unknown signatures are recorded and not guessed or
retried. These calls confirm command acceptance on the candidate build; they do
not establish measured gain change, isolation, internal state survival or
smoothness. `loadfile` is issued again and configured `af` before/after is
recorded, with volume/mute snapshots. A failed install followed by equal empty `af` lists explicitly fails chain
survival. Identical **nonempty** `af` is only configured-chain survival, not proof that internal runtime commands survived reload.

Interface references: [mpv manual (Audio Filters / af-command)](https://mpv.io/manual/stable/)
and [FFmpeg filters](https://ffmpeg.org/ffmpeg-filters.html). Modern documentation
is candidate guidance; the selected packaged version's native errors are the
experimental evidence.

## Verification and acceptance boundary

### Low-level stepped-sine response

Use the standalone `sine_sweep.py` tool for a response tolerance verdict; the
older `probe.py --sweep-response` window-envelope output is diagnostic only and
sets `usable_for_tolerance_verdict=false`. Example, for a 48 kHz all-bands +12 dB
candidate:

```sh
python3 -B scripts/audio-effects/sine_sweep.py \
  --libmpv apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib \
  --manifest apps/desktop/src-tauri/vendor/libmpv/macos/manifest.json \
  --rate 48000 \
  --gains=12,12,12,12,12,12,12,12,12,12 \
  --preamp-db=0 \
  --output /tmp/afx-response.json
```

The tool measures independent sine captures through `0.499 Fs`, with 20 kHz included where representable;
the EQ center-frequency eligibility cutoff remains `0.45 Fs`. Its report checks
the actual rate, stereo channels, frame count and 32-bit PCM format, and compares
measured windows with the composed RBJ response. These reports cover static
low-level response only; they do not verify limiter high-pressure behavior,
runtime transitions, listening quality or cross-platform release acceptance.

The 2026-10-09 macOS capture set is under
`openspec/changes/introduce-audio-effects-equalizer/evidence/macos-stepped-sine-20261009/`.
It contains 52 rate/curve reports (9 built-in curves and 4 stress curves at four
rates), 7,774 frequency comparisons, and records each command/hash. The maximum
measured-versus-RBJ response difference was 0.000006809483 dB; the conservative
rounding bound was 0.000134970 dB. The reports keep `gate_passed=false` and do not
replace the remaining native output and limiter gates.

```sh
python3 -m unittest discover -s scripts/audio-effects -p 'test_*.py' -v
```

Unit tests cover matrix contents, polarity, impulse, requested PCM sample peaks,
license/hashes, repeat generation, invalid inputs, candidate Nyquist exclusions,
node ownership, retained load failures and supervisor timeout. They contain no
fake audio acceptance result.

Every report sets `gate_passed=false`. `ao=null` is not a real output capture.
The PCM mode records integer PCM sample-peak and a one-tone response; integer
output alone cannot establish that an intermediate float chain had no non-finite
values or clipping. Full sweep/pressure response, float-chain diagnostics,
all-finite checks, tool-identified dBTP diagnostics, hardware rebuild, loadfile
parameter-survival, first-sample barrier and timing acceptance remain outstanding.
The Gate still needs 100 repetitions per transition with p95 ≤150 ms,
≤10 ms added progress/seek/EOF offset, underruns, five-minute CPU measurements,
BS.1770-5 loudness-matched listening review and fixed reference machines across
all three platforms. The generated synthetic inputs do not replace licensed
listening material or reviewer records.
