#!/usr/bin/env python3
"""Probe a user-selected distributed libmpv; command success is not a DSP Gate."""
import argparse
import ctypes as C
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time
import struct

from mpv_client import Mpv, decode

CENTERS = (31.25, 62.5, 125, 250, 500, 1000, 2000, 4000, 8000, 16000)
LIMITER = 'alimiter=limit=0.891250938:attack=5:release=50:level_in=1:level_out=1:asc=false:level=false:latency=true'
PROPERTIES = ('mpv-version', 'ffmpeg-version', 'current-ao', 'audio-device',
              'audio-params', 'audio-out-params', 'af', 'volume', 'mute', 'pause', 'time-pos')


def digest(path):
    with path.open('rb') as source:
        checksum = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b''):
            checksum.update(block)
    return checksum.hexdigest()


def read_pcm_wave(path):
    """Read integer PCM from classic or WAVE_FORMAT_EXTENSIBLE RIFF/WAVE."""
    data = None
    fmt = None
    with path.open('rb') as source:
        header = source.read(12)
        if len(header) != 12 or header[:4] != b'RIFF' or header[8:] != b'WAVE':
            raise ValueError('not a RIFF/WAVE file')
        while True:
            chunk_header = source.read(8)
            if not chunk_header:
                break
            if len(chunk_header) != 8:
                raise ValueError('truncated WAVE chunk header')
            name, size = struct.unpack('<4sI', chunk_header)
            chunk = source.read(size)
            if len(chunk) != size:
                raise ValueError('truncated WAVE chunk')
            if name == b'fmt ':
                fmt = chunk
            elif name == b'data':
                data = chunk
            if size & 1:
                source.read(1)
    if fmt is None or data is None or len(fmt) < 16:
        raise ValueError('WAVE file is missing fmt or data chunk')
    format_tag, channels, rate, _, block_align, bits = struct.unpack_from('<HHIIHH', fmt)
    if format_tag == 0xFFFE:
        pcm_guid = bytes.fromhex('0100000000001000800000aa00389b71')
        if len(fmt) < 40 or fmt[24:40] != pcm_guid:
            raise ValueError('unsupported WAVE_FORMAT_EXTENSIBLE subformat')
        format_tag = 1
    if format_tag != 1:
        raise ValueError(f'unsupported WAVE format tag: {format_tag}')
    width = bits // 8
    if channels < 1 or rate < 1 or bits % 8 or width not in (1, 2, 3, 4):
        raise ValueError('unsupported PCM layout')
    if block_align != channels * width or len(data) % block_align:
        raise ValueError('invalid PCM block alignment')
    return channels, width, rate, len(data) // block_align, data


def measure_sine_gain_db(input_pcm, output_pcm, frequency_hz, channels, input_width, output_width, rate):
    """Compare a steady sine's first-channel amplitude after its transient."""
    start = int(0.1 * rate)
    frames = min(len(input_pcm) // (channels * input_width),
                 len(output_pcm) // (channels * output_width))
    if frames <= start:
        raise ValueError('PCM capture is too short for steady-state tone analysis')
    def projection(raw, width):
        real = imaginary = 0.0
        count = frames - start
        scale = float(1 << (width * 8 - 1))
        for frame in range(start, frames):
            offset = (frame * channels) * width
            sample = int.from_bytes(raw[offset:offset + width], 'little', signed=True) / scale
            phase = math.tau * frequency_hz * frame / rate
            real += sample * math.cos(phase)
            imaginary += sample * math.sin(phase)
        return 2.0 * math.hypot(real, imaginary) / count

    source_amplitude = projection(input_pcm, input_width)
    output_amplitude = projection(output_pcm, output_width)
    if source_amplitude <= 0 or output_amplitude <= 0:
        raise ValueError('tone amplitude is zero in source or capture')
    return 20.0 * math.log10(output_amplitude / source_amplitude)


def eq_chain(rate):
    bands = [f'equalizer@eq{i}=f={center:g}:t=q:w={math.sqrt(2):.17g}:g=0:mix=1:normalize=false:precision=f64'
             for i, center in enumerate(CENTERS) if center <= rate * 0.45]
    return '@echo_eq:lavfi=[' + ','.join(['aformat=sample_fmts=dbl', 'volume@preamp=volume=0dB:precision=double', *bands, LIMITER]) + ']'


def individual_eq_chain(rate, gains=None, preamp_db=0.0):
    preamp = 10 ** (preamp_db / 20.0)
    parts = [f'@echo_preamp:lavfi=[aformat=sample_fmts=dblp,volume@preamp=volume={preamp:.15g}:precision=double]']
    gains = gains or {}
    parts.extend(f'@echo_eq{i}:lavfi=[equalizer@eq{i}=f={center:g}:t=q:w={math.sqrt(2):.17g}:g={gains.get(i, 0):.15g}:mix=1:normalize=false:precision=f64]'
                 for i, center in enumerate(CENTERS) if center <= rate * 0.45)
    return ','.join([*parts, '@echo_limiter:lavfi=[' + LIMITER + ']'])


def spatial_chain():
    return '@echo_spatial:lavfi=[aformat=sample_fmts=dbl,volume@preamp=volume=-2.938200260161128dB:precision=double,extrastereo@spatial=m=1.25:c=false,' + LIMITER + ']'


def loaded_libraries():
    """Runtime mappings, not presumed neighboring dependency files."""
    paths = []
    if sys.platform == 'darwin':
        process = C.CDLL(None)
        process._dyld_image_count.restype = C.c_uint32
        process._dyld_get_image_name.argtypes = [C.c_uint32]
        process._dyld_get_image_name.restype = C.c_char_p
        paths = [decode(process._dyld_get_image_name(i)) for i in range(process._dyld_image_count())]
    elif sys.platform.startswith('linux') and Path('/proc/self/maps').exists():
        paths = [line.split()[-1] for line in Path('/proc/self/maps').read_text().splitlines() if '/' in line]
    elif sys.platform == 'win32':
        kernel = C.WinDLL('kernel32', use_last_error=True)
        kernel.GetCurrentProcess.restype = C.c_void_p
        kernel.K32EnumProcessModules.argtypes = [C.c_void_p, C.POINTER(C.c_void_p), C.c_uint32, C.POINTER(C.c_uint32)]
        kernel.K32GetModuleFileNameExW.argtypes = [C.c_void_p, C.c_void_p, C.c_wchar_p, C.c_uint32]
        modules, needed = (C.c_void_p * 2048)(), C.c_uint32()
        handle = kernel.GetCurrentProcess()
        if kernel.K32EnumProcessModules(handle, modules, C.sizeof(modules), C.byref(needed)):
            for module in modules[:min(needed.value // C.sizeof(C.c_void_p), len(modules))]:
                buffer = C.create_unicode_buffer(32768)
                if kernel.K32GetModuleFileNameExW(handle, module, buffer, len(buffer)):
                    paths.append(buffer.value)
    selected = sorted({str(Path(path).resolve()) for path in paths
                       if any(name in Path(path).name.lower() for name in ('mpv', 'avfilter', 'avcodec', 'avformat', 'avutil', 'swresample', 'swscale'))})
    return [{'path': path, 'sha256': digest(Path(path))} for path in selected if Path(path).is_file()]


def snapshot(mpv, label):
    return {'label': label, 'properties': {name: mpv.get(name) for name in PROPERTIES}}


def wait_audio(mpv, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        mpv.pump(0.05)
        if mpv.get('audio-out-params') is not None:
            return True
    return False


def run_probe(args):
    report = {
        'schema_version': 1, 'status': 'probe-only', 'gate_passed': False,
        'platform': platform.platform(), 'architecture': platform.machine(),
        'python_version': platform.python_version(), 'cpu_count': os.cpu_count(),
        'libmpv': {'path': str(args.libmpv.resolve()), 'sha256': digest(args.libmpv)},
        'input': {'path': str(args.input.resolve()), 'sha256': digest(args.input)},
        'command_line': sys.argv, 'ao_requested': args.ao,
        'native_loader': 'Windows ctypes DLL loading' if sys.platform == 'win32' else 'ctypes native dlopen RTLD_LAZY | RTLD_LOCAL, matching Rust libloading; CDLL wraps the same handle',
        'capture': {'performed': args.ao == 'pcm', 'sample_peak_dbfs': None, 'true_peak_dbtp': None,
                    'method': 'libmpv ao=pcm WAVE output after the exact candidate filter chain' if args.ao == 'pcm' else None,
                    'path': str(args.pcm_output.resolve()) if args.ao == 'pcm' else None,
                    'reason': ('PCM AO captures processed samples before CoreAudio; it does not establish device latency, underrun or listening acceptance.'
                               if args.ao == 'pcm' else ('No loopback or hardware capture was performed; CoreAudio properties and command results do not establish response, peak, smoothing or timing acceptance.'
                               if args.ao == 'coreaudio' else 'ao=null discards PCM; properties and command results do not establish response, peak, smoothing or timing acceptance.'))},
        'limitations': ['audio-params describes decoder output, not the intermediate filter environment.',
                        'audio-out-params describes negotiated AO input; debug logs may show filter links. No source metadata is treated as processing proof.',
                        'One host and package only; no hardware rebuild, first-sample barrier, loadfile parameter survival, latency, underrun, CPU budget or listening acceptance.'],
        'client_api': None, 'ffmpeg_exports': None, 'runtime_libraries': [],
        'operations': [], 'events': [], 'snapshots': [], 'checks': [],
    }
    if getattr(args, 'fixture_manifest', None):
        fixture_manifest = json.loads(args.fixture_manifest.read_text(encoding='utf-8'))
        entry = next((entry for entry in fixture_manifest['files'] if entry['file'] == args.input.name), None)
        if not entry or entry['sha256'] != report['input']['sha256']:
            raise ValueError('Fixture manifest has no matching input filename/hash')
        report['input']['fixture'] = entry
        report['input']['generator_sha256'] = fixture_manifest['generator_sha256']
    if args.package:
        report['package'] = {'path': str(args.package.resolve()), 'sha256': digest(args.package)}
    if args.manifest:
        report['distribution_manifest'] = {'path': str(args.manifest.resolve()), 'sha256': digest(args.manifest),
                                           'contents': json.loads(args.manifest.read_text(encoding='utf-8'))}
        report['distribution_files'] = []
        for filename, expected in report['distribution_manifest']['contents'].get('files', {}).items():
            path = args.libmpv.resolve().parent / filename
            actual = digest(path) if path.is_file() else None
            report['distribution_files'].append({'path': str(path), 'sha256': actual,
                                                  'expected_sha256': expected, 'hash_matches_manifest': actual == expected,
                                                  'meaning': 'Neighboring distributed file hash; not proof it loaded.'})
    try:
        mpv = Mpv(args.libmpv.resolve(), args.ao, args.pcm_output)
    except Exception as error:
        report.update(status='failed', error=f'{type(error).__name__}: {error}', startup_failed_before_native_properties=True)
        return report
    try:
        version = mpv.lib.mpv_client_api_version()
        report['client_api'] = {'raw': version, 'major': version >> 16, 'minor': version & 0xffff}
        report['runtime_libraries'] = loaded_libraries()
        # Optional dependency exports reached through this exact libmpv handle.
        report['ffmpeg_exports'] = {}
        for name in ('av_version_info', 'avfilter_version', 'avcodec_version', 'avutil_version'):
            try:
                function = getattr(mpv.lib, name)
                function.argtypes = []
                function.restype = C.c_char_p if name == 'av_version_info' else C.c_uint
                value = function()
                report['ffmpeg_exports'][name] = decode(value) if name == 'av_version_info' else value
            except AttributeError:
                report['ffmpeg_exports'][name] = None
        report['filter_availability'] = {}
        try:
            find_filter = mpv.lib.avfilter_get_by_name
            find_filter.argtypes, find_filter.restype = [C.c_char_p], C.c_void_p
            for name in ('aformat', 'volume', 'equalizer', 'extrastereo', 'alimiter'):
                report['filter_availability'][name] = bool(find_filter(name.encode()))
        except AttributeError:
            report['filter_availability']['error'] = 'avfilter_get_by_name export unavailable'
        if args.capture_only:
            input_channels, input_width, input_rate, _, _ = read_pcm_wave(args.input)
            capture_chain = individual_eq_chain(input_rate, {5: args.capture_gain_db}, args.capture_preamp_db)
            chain_result = mpv.command('af', 'set', capture_chain)
            mpv.command('loadfile', str(args.input.resolve()))
            ready = wait_audio(mpv, args.step_timeout)
            source = mpv.get('audio-params')
            rate = source.get('samplerate') if isinstance(source, dict) else None
            if not ready or not rate or chain_result < 0:
                report['checks'].append({'name': 'capture_setup', 'audio_ready': ready,
                                         'chain_result': chain_result,
                                         'error': 'Capture chain or native output did not initialize; response is not measured.'})
                return report
            report['candidate_rate_basis'] = {'rate': rate, 'source': 'input PCM WAVE format checked against negotiated decoder rate',
                                              'input_channels': input_channels, 'input_sample_width_bytes': input_width}
            report['snapshots'].append(snapshot(mpv, 'capture_chain_installed'))
            duration = mpv.get('duration')
            report['checks'].append({'name': 'processed_pcm_capture', 'chain_result': chain_result,
                                     'audio_ready': ready, 'duration_seconds': duration,
                                     'gain_db': args.capture_gain_db, 'preamp_db': args.capture_preamp_db,
                                     'chain': capture_chain,
                                     'meaning': 'One isolated candidate chain was captured; this proves its processed PCM response only for this host/input/rate.'})
            return report
        mpv.command('loadfile', str(args.input.resolve()))
        report['checks'].append({'name': 'initial_audio_ready', 'observed': wait_audio(mpv, args.step_timeout)})
        mpv.set('pause', 'yes')
        report['snapshots'].append(snapshot(mpv, 'bypass'))
        source = mpv.get('audio-params')
        rate = source.get('samplerate') if isinstance(source, dict) else None
        if not rate:
            report['checks'].append({'name': 'candidate_setup', 'error': 'No decoder sample rate; candidate not guessed.'})
            return report
        report['candidate_rate_basis'] = {'rate': rate, 'source': 'decoder audio-params; candidate input only, not processing confirmation'}
        for name, chain in [('eq', eq_chain(rate)), ('eq_individual', individual_eq_chain(rate)), ('spatial', spatial_chain())]:
            if name == 'spatial' and isinstance(source, dict) and source.get('channel-count') != 2:
                report['checks'].append({'name': 'spatial', 'skipped': 'Only stereo is eligible; probe refuses automatic mono upmix/downmix.'})
                continue
            result = mpv.command('af', 'set', chain)
            mpv.set('pause', 'no')
            mpv.pump(0.15)
            mpv.set('pause', 'yes')
            report['snapshots'].append(snapshot(mpv, f'{name}_installed'))
            report['checks'].append({'name': f'{name}_chain', 'chain': chain, 'command_result': result,
                                    'meaning': 'Command acceptance only; inspect logs and audio-out-params for initialization failures.'})
            if name.startswith('eq'):
                # Try exact four-argument instance target and older lavfi direct command.
                # Errors are evidence; no fallback is silently called successful.
                commands = [('af-command', 'echo_eq', 'eq5:gain', '1'),
                                ('af-command', 'echo_eq', 'preamp:volume', '0.501187233627272')] if name == 'eq' else [
                                ('af-command', 'echo_eq5', 'eq5:gain', '1'),
                                ('af-command', 'echo_preamp', 'preamp:volume', '0.501187233627272')]
                for command in commands:
                    code = mpv.command(*command)
                    mpv.pump(0.1)
                    report['checks'].append({'name': 'af_command_syntax', 'args': list(command), 'result': code,
                                            'meaning': 'Acceptance does not prove parameter change, isolation, atomicity or smoothing.'})
            if name == 'spatial':
                command = ('af-command', 'echo_spatial', 'spatial:m', '1.25')
                code = mpv.command(*command)
                mpv.pump(0.1)
                report['checks'].append({'name': 'spatial_filter_command', 'args': list(command), 'result': code,
                                         'meaning': 'Acceptance does not prove parameter change, isolation, atomicity or smoothing.'})
            before = mpv.get('af')
            mpv.command('loadfile', str(args.input.resolve()))
            mpv.set('pause', 'no')
            ready = wait_audio(mpv, args.step_timeout)
            mpv.pump(0.1)
            mpv.set('pause', 'yes')
            after = mpv.get('af')
            report['checks'].append({'name': f'{name}_loadfile_chain_survival', 'audio_ready': ready,
                                    'before': before, 'after': after, 'configured_chain_equal': before == after,
                                    'configured_chain_survived': result >= 0 and bool(before) and before == after,
                                    'meaning': 'Equal empty chains cannot establish survival. Nonempty configured-chain survival still does not establish updated internal parameter survival without capture.'})
            report['snapshots'].append(snapshot(mpv, f'{name}_after_loadfile'))
        report['candidate_chains_accepted'] = all(check['command_result'] >= 0 for check in report['checks'] if check['name'].endswith('_chain'))
        if args.ao == 'pcm':
            # Capture the exact labeled-filter command shape used by NativeEffects:
            # aformat -> preamp -> individually named EQ stages -> limiter.
            capture_chain = individual_eq_chain(rate)
            chain_result = mpv.command('af', 'set', capture_chain)
            preamp_result = mpv.command('af-command', 'echo_preamp', 'preamp:volume', '0.446683592150963')
            band_result = mpv.command('af-command', 'echo_eq5', 'eq5:gain', '6')
            mpv.command('loadfile', str(args.input.resolve()))
            ready = wait_audio(mpv, args.step_timeout)
            duration = mpv.get('duration')
            mpv.set('pause', 'no')
            mpv.pump(max(2.5, float(duration or 2.0) + 0.25))
            mpv.set('pause', 'yes')
            report['checks'].append({'name': 'processed_pcm_capture', 'chain_result': chain_result,
                                    'preamp_result': preamp_result, 'band_result': band_result,
                                    'gain_db': 6.0, 'preamp_db': -7.0,
                                    'audio_ready': ready, 'duration_seconds': duration,
                                    'chain': capture_chain,
                                    'meaning': 'The captured WAVE is the processed native candidate output after runtime in-place EQ and preamp commands; it does not model CoreAudio device conversion or latency.'})
        mpv.command('af', 'set', '')
        mpv.pump(0.1)
        report['snapshots'].append(snapshot(mpv, 'removed'))
        return report
    except Exception as error:
        report.update(status='failed', error=f'{type(error).__name__}: {error}')
        return report
    finally:
        report['operations'], report['events'] = mpv.operations, mpv.events
        mpv.close()
        if args.ao == 'pcm' and args.pcm_output.is_file():
            report['capture']['sha256'] = digest(args.pcm_output)
            try:
                channels, width, rate, frames, raw = read_pcm_wave(args.pcm_output)
                peak = 0.0
                if width == 1:
                    peak = max((abs(value - 128) / 128.0 for value in raw), default=0.0)
                elif width == 2:
                    peak = max((abs(int.from_bytes(raw[index:index + 2], 'little', signed=True)) / 32768.0
                                for index in range(0, len(raw) - 1, 2)), default=0.0)
                elif width == 3:
                    samples = (int.from_bytes(raw[index:index + 3], 'little', signed=False)
                               for index in range(0, len(raw) - 2, 3))
                    peak = max((abs(sample - (1 << 24) if sample & (1 << 23) else sample) / 8388608.0
                                for sample in samples), default=0.0)
                elif width == 4:
                    peak = max((abs(int.from_bytes(raw[index:index + 4], 'little', signed=True)) / 2147483648.0
                                for index in range(0, len(raw) - 3, 4)), default=0.0)
                report['capture'].update(channels=channels, sample_rate=rate, sample_width_bytes=width,
                                         frames=frames, sample_format='integer PCM', all_finite=True,
                                         sample_peak_dbfs=(20 * math.log10(peak) if peak > 0 else None))
                fixture = report.get('input', {}).get('fixture', {})
                capture_check = next((check for check in report['checks']
                                      if check.get('name') == 'processed_pcm_capture'), {})
                if (fixture.get('kind') == 'tone997'
                        and fixture.get('requested_sample_peak_dbfs', 0) <= -40
                        and channels == fixture.get('channels') and rate == fixture.get('rate_hz')):
                    input_channels, input_width, input_rate, _, input_raw = read_pcm_wave(args.input)
                    if input_channels == channels and input_rate == rate:
                        expected_db = float(capture_check.get('gain_db', 0.0)) + float(capture_check.get('preamp_db', 0.0))
                        measured_db = measure_sine_gain_db(
                            input_raw, raw, 997.0, channels, input_width, width, rate)
                        target_db = expected_db
                        report['capture']['tone_response'] = {
                            'frequency_hz': 997.0, 'target_center_hz': 1000.0,
                            'eq_gain_db': float(capture_check.get('gain_db', 0.0)),
                            'preamp_db': float(capture_check.get('preamp_db', 0.0)),
                            'target_net_gain_db': target_db, 'measured_gain_db': measured_db,
                            'error_db': measured_db - target_db,
                            'measurement': 'steady-state first-channel sinusoidal projection, 100 ms transient excluded; expected net includes runtime EQ and preamp commands',
                        }
            except (EOFError, ValueError, struct.error) as error:
                report['capture']['analysis_error'] = f'{type(error).__name__}: {error}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--libmpv', type=Path, required=True, help='Exact distributed dylib/DLL/so. No system library fallback.')
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--fixture-manifest', type=Path, help='Generated manifest; verify and record input license/hash.')
    parser.add_argument('--package', type=Path, help='Optional installer/archive to hash (must be a file).')
    parser.add_argument('--ao', default='null', choices=('null', 'coreaudio', 'pcm'),
                        help='null discards PCM; coreaudio plays at 1%% host volume; pcm writes processed output to a WAVE file.')
    parser.add_argument('--pcm-output', type=Path, help='Required with --ao pcm; captures processed libmpv output.')
    parser.add_argument('--capture-only', action='store_true', help='Capture one isolated static EQ chain; requires --ao pcm.')
    parser.add_argument('--capture-gain-db', type=float, default=6.0,
                        help='Gain for the 1 kHz capture band when --capture-only is used (default: 6 dB).')
    parser.add_argument('--capture-preamp-db', type=float, default=0.0,
                        help='Static preamp for the capture chain (default: 0 dB).')
    parser.add_argument('--step-timeout', type=float, default=3)
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--worker', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if any(not math.isfinite(value) or not 0.1 <= value <= 120 for value in (args.timeout, args.step_timeout)):
        parser.error('timeouts must be finite and between 0.1 and 120 seconds')
    for path in (args.libmpv, args.input, args.manifest, args.package, args.fixture_manifest):
        if path and not path.is_file():
            parser.error(f'Not a file: {path}')
    if args.ao == 'pcm' and not args.pcm_output:
        parser.error('--pcm-output is required with --ao pcm')
    if args.ao != 'pcm' and args.pcm_output:
        parser.error('--pcm-output can only be used with --ao pcm')
    if args.capture_only and args.ao != 'pcm':
        parser.error('--capture-only requires --ao pcm')
    if args.ao == 'pcm' and not args.capture_only:
        parser.error('--ao pcm only supports isolated --capture-only; use --ao coreaudio for time-sensitive runtime-command checks')
    if args.capture_only and not args.input.name.lower().endswith('.wav'):
        parser.error('--capture-only currently requires a PCM WAVE input fixture')
    if not math.isfinite(args.capture_gain_db) or not -12 <= args.capture_gain_db <= 12:
        parser.error('--capture-gain-db must be finite and within -12 to 12')
    if not math.isfinite(args.capture_preamp_db) or not -60 <= args.capture_preamp_db <= 12:
        parser.error('--capture-preamp-db must be finite and within -60 to 12')
    if args.pcm_output:
        args.pcm_output = args.pcm_output.resolve()
        args.pcm_output.parent.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.worker:
        try:
            report = run_probe(args)
        except Exception as error:
            report = {'schema_version': 1, 'status': 'failed', 'gate_passed': False,
                      'error': f'{type(error).__name__}: {error}', 'libmpv_path': str(args.libmpv.resolve())}
        args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
        return 1 if report['status'] == 'failed' else 0
    # Native calls including initialize/destroy cannot stall the supervising process.
    with tempfile.TemporaryDirectory(prefix='echo-mpv-probe-') as temporary:
        worker_output = Path(temporary) / 'report.json'
        arguments = sys.argv[1:]
        for index, argument in enumerate(arguments):
            if argument == '--output':
                arguments[index + 1] = str(worker_output)
                break
            if argument.startswith('--output='):
                arguments[index] = '--output=' + str(worker_output)
                break
        try:
            result = subprocess.run([sys.executable, str(Path(__file__).resolve()), *arguments, '--worker'],
                                    capture_output=True, text=True, timeout=args.timeout)
            if worker_output.exists():
                report = json.loads(worker_output.read_text(encoding='utf-8'))
            else:
                report = {'status': 'failed', 'gate_passed': False, 'error': f'Native worker exited {result.returncode} without a report'}
            report['worker_exit_code'] = result.returncode
            report['worker_stderr'] = result.stderr
        except subprocess.TimeoutExpired as error:
            report = {'status': 'timeout', 'gate_passed': False, 'timeout_seconds': args.timeout,
                      'error': 'Native subprocess terminated after deadline', 'worker_stderr': decode(error.stderr)}
    report.setdefault('libmpv', {'path': str(args.libmpv.resolve()), 'sha256': digest(args.libmpv)})
    report.setdefault('input', {'path': str(args.input.resolve()), 'sha256': digest(args.input)})
    report['invocation'] = [sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]]
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(f"{report['status']}: {args.output}; gate_passed=false")
    return 0 if report['status'] == 'probe-only' else 1


if __name__ == '__main__':
    sys.exit(main())
