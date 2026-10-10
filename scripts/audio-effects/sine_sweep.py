#!/usr/bin/env python3
"""Measure a controlled-rate static EQ curve with matched native PCM captures.

This is a response experiment, never the complete audio-effects/native Gate.
Only generated low-level stereo s32 sine fixtures and typed EQ parameters are
accepted. No audio device is opened. Each segment settles for >=24 cycles and
uses >=32 cycles for a least-squares sine/cosine/DC fit (no FFT-bin assumption).
"""
import argparse
import json
import math
from pathlib import Path
import platform
import struct
import subprocess
import sys
import tempfile
import time
import wave

from mpv_client import Mpv
from probe import CENTERS, digest, individual_eq_chain, loaded_libraries, read_pcm_wave


def frequencies(rate, count=120):
    if count < 120:
        raise ValueError('at least 120 logarithmic frequency points are required')
    lower, upper = 20.0, 0.499 * rate
    points = {lower * (upper / lower) ** (i / (count - 1)) for i in range(count)}
    # Center and half-octave neighbors: cover overlap and warped high bands.
    for center in CENTERS:
        if center <= 0.45 * rate:
            points.update(f for f in (center / math.sqrt(2), center, center * math.sqrt(2))
                          if lower <= f <= upper)
    if 20000 <= upper:
        points.add(20000.0)
    return sorted(points)


def curve_response_db(gains, rate, frequency, preamp_db=0):
    z = complex(math.cos(-math.tau * frequency / rate), math.sin(-math.tau * frequency / rate))
    response = complex(1)
    for center, gain in zip(CENTERS, gains):
        if center > 0.45 * rate or gain == 0:
            continue
        a = 10 ** (gain / 40)
        w = math.tau * center / rate
        alpha = math.sin(w) / (2 * math.sqrt(2))
        b0, b1, b2 = 1 + alpha * a, -2 * math.cos(w), 1 - alpha * a
        a0, a1, a2 = 1 + alpha / a, -2 * math.cos(w), 1 - alpha / a
        response *= (b0 + b1 * z + b2 * z * z) / (a0 + a1 * z + a2 * z * z)
    return 20 * math.log10(abs(response)) + preamp_db


def generate_tones(path, rate, points, level_dbfs=-60):
    amplitude = 10 ** (level_dbfs / 20)
    windows, cursor = [], 0
    with wave.open(str(path), 'wb') as target:
        target.setparams((2, 4, rate, 0, 'NONE', 'not compressed'))
        for frequency in points:
            settle = math.ceil(max(0.25, 24 / frequency) * rate)
            measured = math.ceil(max(0.25, 32 / frequency) * rate)
            count = settle + measured
            raw = bytearray(count * 8)
            for frame in range(count):
                value = round(amplitude * math.sin(math.tau * frequency * frame / rate) * (2 ** 31 - 1))
                struct.pack_into('<ii', raw, frame * 8, value, value)
            target.writeframesraw(raw)
            windows.append({'frequency_hz': frequency, 'segment_start_frame': cursor,
                            'settle_frames': settle, 'window_frames': [cursor + settle, cursor + count]})
            cursor += count
    return windows, cursor


def solve3(matrix, rhs):
    rows = [list(row) + [value] for row, value in zip(matrix, rhs)]
    for column in range(3):
        pivot = max(range(column, 3), key=lambda i: abs(rows[i][column]))
        rows[column], rows[pivot] = rows[pivot], rows[column]
        scale = rows[column][column]
        if abs(scale) < 1e-10:
            raise ValueError('degenerate sine-fit window')
        rows[column] = [value / scale for value in rows[column]]
        for row in range(3):
            if row != column:
                factor = rows[row][column]
                rows[row] = [a - factor * b for a, b in zip(rows[row], rows[column])]
    return [row[3] for row in rows]


def project_s32(raw, channels, rate, frequency, start, end, phase_origin=0):
    if channels != 2 or end > len(raw) // 8 or start < 0 or end <= start:
        raise ValueError('invalid or truncated stereo measurement window')
    if (end - start) * frequency / rate < 31.99:
        raise ValueError('measurement window needs at least 32 cycles')
    sums = [0.0] * 9
    peak = 0.0
    for frame in range(start, end):
        sample = struct.unpack_from('<i', raw, frame * 8)[0] / 2 ** 31
        sine = math.sin(math.tau * frequency * (frame - phase_origin) / rate)
        cosine = math.cos(math.tau * frequency * (frame - phase_origin) / rate)
        sums[0] += sine * sine
        sums[1] += cosine * cosine
        sums[2] += sine * cosine
        sums[3] += sine
        sums[4] += cosine
        sums[5] += sample * sine
        sums[6] += sample * cosine
        sums[7] += sample
        sums[8] += sample * sample
        peak = max(peak, abs(sample))
    ss, cc, sc, s, c, ys, yc, y, energy = sums
    a, b, dc = solve3([[ss, sc, s], [sc, cc, c], [s, c, end - start]], [ys, yc, y])
    amplitude = math.hypot(a, b)
    if amplitude <= 0:
        raise ValueError('zero measured amplitude')
    # A deliberately conservative bound for rounding error in the fitted
    # amplitude. The fit has >=32 cycles, hence well-conditioned sine columns.
    rounding_bound_db = 20 * math.log10(1 + 4 / (2 ** 31 * amplitude))
    if rounding_bound_db > 0.01:
        raise ValueError('s32 quantization bound exceeds 0.01 dB; increase fixture level')
    if peak >= 10 ** (-12 / 20):
        raise ValueError('low-level experiment exceeded -12 dBFS; limiter/clipping exclusion failed')
    residual_rms = math.sqrt(max(0, energy - a * ys - b * yc - dc * y) / (end - start))
    if residual_rms / amplitude > 0.001:
        raise ValueError('sine-fit residual exceeds 0.1%; settling, nonlinearity or contamination suspected')
    return {'amplitude': amplitude, 'dc': dc, 'sample_peak_dbfs': 20 * math.log10(peak),
            'quantization_bound_db': rounding_bound_db, 'residual_rms_relative_to_amplitude': residual_rms / amplitude}


def validate_capture(path, rate, frames):
    channels, width, output_rate, output_frames, raw = read_pcm_wave(path)
    if (channels, width, output_rate, output_frames) != (2, 4, rate, frames):
        raise ValueError(f'capture must be stereo s32 with matching rate and exact frames; got '
                         f'{channels=}, {width=}, {output_rate=}, {output_frames=}, expected {rate=}, {frames=}')
    # The container width alone is insufficient: extensible PCM may carry only
    # 24 valid bits inside a 32-bit word, invalidating the rounding bound.
    with path.open('rb') as source:
        source.seek(12)
        while True:
            name, size = struct.unpack('<4sI', source.read(8))
            if name == b'fmt ':
                fmt = source.read(size)
                if struct.unpack_from('<H', fmt)[0] == 0xFFFE and struct.unpack_from('<H', fmt, 18)[0] != 32:
                    raise ValueError('capture must contain 32 valid PCM bits, not only a 32-bit container')
                break
            source.seek(size + (size & 1), 1)
    return raw


def capture(library, source, destination, chain, timeout):
    mpv = Mpv(library, ao='pcm', ao_pcm_file=destination, audio_format='s32')
    try:
        if chain and mpv.command('af', 'set', chain) < 0:
            raise RuntimeError('candidate chain rejected')
        if mpv.command('loadfile', str(source)) < 0:
            raise RuntimeError('generated fixture rejected')
        deadline = time.monotonic() + timeout
        params = None
        while time.monotonic() < deadline:
            mpv.pump(0.01)
            current = mpv.get('audio-out-params')
            if current:
                params = {'decoder': mpv.get('audio-params'), 'output': current}
            if mpv.get('eof-reached'):
                break
        else:
            raise RuntimeError('capture did not reach EOF within timeout')
        failures = [event for event in mpv.events if event.get('level') in ('error', 'fatal')]
        if failures:
            raise RuntimeError(f'native capture logged errors: {failures}')
        return {'params': params, 'operations': mpv.operations,
                'runtime_libraries': loaded_libraries(),
                'versions': {key: mpv.get(key) for key in ('mpv-version', 'ffmpeg-version')},
                'filter_logs': [event for event in mpv.events if event.get('prefix', '').startswith(('lavfi', 'af'))]}
    finally:
        mpv.close()


def provenance():
    root = Path(__file__).resolve().parents[2]
    def git(*args):
        return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()
    paths = ('scripts/audio-effects/sine_sweep.py', 'scripts/audio-effects/mpv_client.py',
             'scripts/audio-effects/probe.py', 'crates/echo-desktop/src/effects/math.rs',
             'crates/echo-desktop/src/player/native_effects.rs')
    return {'git_head': git('rev-parse', 'HEAD'), 'git_status_porcelain': git('status', '--porcelain'),
            'source_sha256': {name: digest(root / name) for name in paths}}


def run(args):
    points = frequencies(args.rate, args.points)
    report = {'schema': 'echo-static-stepped-sine-response-v1', 'gate_passed': False,
              'platform': platform.platform(), 'rate_hz': args.rate, 'gains_db': args.gains,
              'preamp_db': args.preamp_db, 'fixture_level_dbfs': args.level_dbfs,
              'libmpv': {'path': str(args.libmpv.resolve()), 'sha256': digest(args.libmpv)},
              'provenance': provenance(), 'status': 'failed', 'response_passed': False,
              'scope': 'Static controlled-rate PCM AO response only; no hardware, actor transitions, CPU, true-peak, listening or cross-platform Gate.',
              'rate_basis': 'aformat explicitly constrains the diagnostic EQ graph to rate_hz; this does not prove the product actor uses its actual filter link rate.',
              'response_tolerance_db': 0.25, 'frequency_points': len(points)}
    if args.manifest:
        report['manifest'] = {'path': str(args.manifest.resolve()), 'sha256': digest(args.manifest),
                              'content': json.loads(args.manifest.read_text())}
    try:
        with tempfile.TemporaryDirectory(prefix='echo-sine-response-') as directory:
            root = Path(directory)
            source, bypass, candidate = (root / name for name in ('source.wav', 'bypass.wav', 'candidate.wav'))
            windows, frames = generate_tones(source, args.rate, points, args.level_dbfs)
            report['fixture'] = {'sha256': digest(source), 'frames': frames, 'license': 'CC0-1.0',
                                 'generation': 'deterministic s32 stereo stepped sine, 24 settle cycles/32 fit cycles minimum'}
            graph = individual_eq_chain(args.rate, dict(enumerate(args.gains)), args.preamp_db)
            graph = graph.replace('sample_fmts=dblp', f'sample_fmts=dblp:sample_rates={args.rate}', 1)
            report['graph'] = graph
            report['bypass_native'] = capture(args.libmpv.resolve(), source, bypass, None, args.timeout)
            report['candidate_native'] = capture(args.libmpv.resolve(), source, candidate, graph, args.timeout)
            baseline, processed = validate_capture(bypass, args.rate, frames), validate_capture(candidate, args.rate, frames)
            report['captures'] = {'bypass_sha256': digest(bypass), 'candidate_sha256': digest(candidate),
                                  'encoding': 'verified signed integer PCM, stereo, 32-bit', 'frames': frames}
            results = []
            for window in windows:
                start, end = window['window_frames']
                frequency = window['frequency_hz']
                before = project_s32(baseline, 2, args.rate, frequency, start, end, window['segment_start_frame'])
                after = project_s32(processed, 2, args.rate, frequency, start, end, window['segment_start_frame'])
                bypass_error = 20 * math.log10(before['amplitude']) - args.level_dbfs
                if abs(bypass_error) > 0.01:
                    raise ValueError(f'bypass does not reproduce the generated sine level: {bypass_error} dB')
                measured = 20 * math.log10(after['amplitude'] / before['amplitude'])
                expected = curve_response_db(args.gains, args.rate, frequency, args.preamp_db)
                results.append({**window, 'bypass': before, 'candidate': after, 'measured_db': measured,
                                'bypass_fixture_level_error_db': bypass_error,
                                'expected_db': expected, 'error_db': measured - expected})
            report['results'] = results
            report['maximum_absolute_error_db'] = max(abs(point['error_db']) for point in results)
            report.update(status='measured', response_passed=report['maximum_absolute_error_db'] <= 0.25)
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    return report


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--libmpv', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--rate', type=int, choices=(22050, 44100, 48000, 96000), required=True)
    parser.add_argument('--gains', type=lambda value: [float(x) for x in value.split(',')],
                        default=[0, 0, 0, 0, 0, 6, 0, 0, 0, 0], help='Ten comma-separated gains in dB')
    parser.add_argument('--preamp-db', type=float, default=0)
    parser.add_argument('--level-dbfs', type=float, default=-60)
    parser.add_argument('--points', type=int, default=120)
    parser.add_argument('--timeout', type=float, default=60, help='Each native capture timeout in seconds')
    parser.add_argument('--worker', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if len(args.gains) != 10 or any(not math.isfinite(value) or not -12 <= value <= 12 for value in args.gains):
        parser.error('--gains requires exactly ten finite values within -12 to 12')
    for name, lower, upper in (('preamp_db', -60, 12), ('level_dbfs', -90, -30), ('timeout', 1, 120)):
        value = getattr(args, name)
        if not math.isfinite(value) or not lower <= value <= upper:
            parser.error(f'--{name.replace("_", "-")} must be finite and within {lower} to {upper}')
    if args.points < 120 or args.points > 2000:
        parser.error('--points must be within 120 to 2000')
    for path in (args.libmpv, args.manifest):
        if path and not path.is_file():
            parser.error(f'not a file: {path}')
    return args


def main():
    args = parse_args()
    if not args.worker:
        # The native wait/decode call can itself block; isolate the entire
        # measurement in a bounded subprocess, rather than trust its loop.
        try:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.unlink(missing_ok=True)
            result = subprocess.run([sys.executable, '-B', str(Path(__file__).resolve()),
                                     *sys.argv[1:], '--worker'], timeout=2 * args.timeout + 120,
                                    capture_output=True, text=True)
            print(result.stdout, end='')
            if result.stderr:
                print(result.stderr, file=sys.stderr, end='')
            if not args.output.exists():
                args.output.write_text(json.dumps({'schema': 'echo-static-stepped-sine-response-v1',
                                                   'status': 'worker-failed', 'response_passed': False,
                                                   'gate_passed': False, 'provenance': provenance(),
                                                   'error': f'native worker exited {result.returncode} without a report'}, indent=2) + '\n')
                return 1
            return result.returncode
        except subprocess.TimeoutExpired:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps({'schema': 'echo-static-stepped-sine-response-v1',
                                               'status': 'timeout', 'response_passed': False,
                                               'gate_passed': False, 'provenance': provenance(),
                                               'error': 'native measurement worker exceeded bounded timeout'}, indent=2) + '\n')
            return 1
    report = run(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    print(json.dumps({key: report.get(key) for key in ('status', 'response_passed', 'maximum_absolute_error_db', 'gate_passed', 'error')}))
    return 0 if report['response_passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
