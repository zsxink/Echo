#!/usr/bin/env python3
"""Offline, standard-library-only synthetic native Gate inputs (PCM32 WAV)."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import struct
import wave

RATES = (22050, 44100, 48000, 96000)
KINDS = ('tone997', 'sweep', 'noise', 'impulse', 'left-only', 'antiphase')
LEVELS = (-0.1, -60.0)
SEED = 0x4543484F
LICENSE = 'CC0-1.0'


def samples(kind, rate, frames):
    """Own xorshift32 noise; other signals use Python's platform libm."""
    state = SEED
    duration = frames / rate
    start, stop = 20.0, min(20000.0, rate * 0.45)
    ratio = math.log(stop / start)
    for index in range(frames):
        time = index / rate
        if kind == 'noise':
            state ^= (state << 13) & 0xffffffff
            state ^= state >> 17
            state ^= (state << 5) & 0xffffffff
            state &= 0xffffffff
            value = (state / 0xffffffff) * 2 - 1
        elif kind == 'impulse':
            value = 1.0 if index == frames // 4 else 0.0
        elif kind == 'sweep':
            phase = 2 * math.pi * start * duration / ratio * math.expm1(ratio * time / duration)
            value = math.sin(phase)
        else:
            value = math.sin(2 * math.pi * 997 * time)
        yield value


def write_fixture(path, kind, rate, channels, level, duration):
    frames = round(rate * duration)
    values = list(samples(kind, rate, frames))
    peak = max(abs(value) for value in values)
    scale = (10 ** (level / 20)) / peak
    encoded = bytearray()
    sample_peak = 0
    for value in values:
        sample = round(value * scale * 2147483647)
        sample_peak = max(sample_peak, abs(sample))
        frame = (sample,) if channels == 1 else (sample, sample)
        if kind == 'left-only':
            frame = (sample, 0)
        elif kind == 'antiphase':
            frame = (sample, -sample)
        encoded.extend(struct.pack('<' + 'i' * channels, *frame))
    with wave.open(str(path), 'wb') as output:
        output.setnchannels(channels)
        output.setsampwidth(4)
        output.setframerate(rate)
        output.writeframes(encoded)
    return {
        'file': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
        'source': 'Echo synthetic signal; no third-party recordings', 'license': LICENSE,
        'kind': kind, 'rate_hz': rate, 'channels': channels, 'frames': frames,
        'duration_seconds': frames / rate, 'requested_sample_peak_dbfs': level,
        'quantized_sample_peak_dbfs': 20 * math.log10(sample_peak / 2147483648),
        'sample_format': 'signed PCM32 little-endian',
    }


def generate(output, duration=2.0, rates=RATES):
    if not math.isfinite(duration) or not 0.05 <= duration <= 60:
        raise ValueError('duration must be finite and between 0.05 and 60 seconds')
    if any(rate not in RATES for rate in rates):
        raise ValueError('only 22050, 44100, 48000 and 96000 Hz are supported')
    output.mkdir(parents=True, exist_ok=True)
    entries = []
    for rate in rates:
        for channels in (1, 2):
            for kind in KINDS:
                if channels == 1 and kind in ('left-only', 'antiphase'):
                    continue  # These tests have meaning only with two independent channels.
                for level in LEVELS:
                    name = f'{kind}-{rate}-{channels}ch-{abs(level):g}db.wav'
                    entries.append(write_fixture(output / name, kind, rate, channels, level, duration))
    manifest = {
        'schema_version': 1, 'generator_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'python_version': platform.python_version(), 'noise_algorithm': 'xorshift32', 'noise_seed': SEED,
        'license': LICENSE, 'license_url': 'https://creativecommons.org/publicdomain/zero/1.0/',
        'reproducibility': 'Same inputs and runtime produce identical bytes. libm rounding may vary across platforms; compare manifest hashes before reusing measurements.',
        'sweep': {'start_hz': 20, 'stop_hz': 'min(20000, 0.45 * rate)', 'scale': 'logarithmic'},
        'files': entries,
    }
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--duration', type=float, default=2)
    parser.add_argument('--rates', type=int, nargs='+', choices=RATES, default=RATES)
    args = parser.parse_args()
    manifest = generate(args.output, args.duration, args.rates)
    print(f"Generated {len(manifest['files'])} WAV files and manifest.json in {args.output}")


if __name__ == '__main__':
    main()
