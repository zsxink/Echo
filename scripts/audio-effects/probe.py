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
import re
import subprocess
import sys
import tempfile
import time
import struct
import wave

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


def af_command_signature(mpv):
    """Return the loaded library's af-command positional argument count."""
    commands = mpv.get('command-list')
    if not isinstance(commands, list):
        return None
    for command in commands:
        if isinstance(command, dict) and command.get('name') == 'af-command':
            arguments = command.get('args')
            return len(arguments) if isinstance(arguments, list) else None
    return None


def af_command_args(argument_count, label, target, option, value):
    """Build the syntax reported by this library; never guess or retry."""
    if argument_count == 3:
        return ('af-command', label, f'{target}:{option}', value)
    if argument_count == 4:
        return ('af-command', label, option, value, target)
    return None


def graph_text(mpv):
    """Flatten the configured `af` graph to a comparable string.

    mpv accepts a runtime command for a named filter and still reports rc=0 even
    when the filter has no `process_command`, because f_lavfi.c only tests
    `result >= 0`. The configured graph does not report runtime parameter state; only
    captured PCM can establish an actual change.
    """
    entries = mpv.get('af')
    if not isinstance(entries, list):
        return None
    graphs = []
    for entry in entries:
        if isinstance(entry, dict):
            params = entry.get('params')
            if isinstance(params, dict) and isinstance(params.get('graph'), str):
                graphs.append(params['graph'])
    return '\n'.join(graphs) if graphs else None


def option_value(graph, key):
    """Return the last `key=value` in a filter graph string, or None.

    Handles both bare filters (`m=1.25`) and labelled ones
    (`extrastereo@spatial=m=1.25:c=false`).
    """
    if not graph:
        return None
    matches = re.findall(rf'(?:^|[@:;,]){re.escape(key)}=([^:,;\]]+)', graph)
    return matches[-1] if matches else None


def runtime_parameter_probe(mpv, argument_count, label, target, option, value):
    """Send one runtime command and record its configured graph, without a DSP verdict.

    The caller must pass a `value` that DIFFERS from the installed one, otherwise
    a no-op and a real change are indistinguishable. Returns a record carrying
    the before/after graph so the verdict is auditable.
    """
    before = graph_text(mpv)
    command = af_command_args(argument_count, label, target, option, value)
    if not command:
        return {'args': None, 'result': None, 'skipped': 'unsupported af-command signature',
                'before': before, 'after': None, 'changed': False,
                'before_value': None, 'after_value': None, 'requested': value,
                'meaning': 'No command was issued, so nothing was established.'}
    code = mpv.command(*command)
    mpv.pump(0.15)
    after = graph_text(mpv)
    before_value = option_value(before, option)
    after_value = option_value(after, option)
    return {'args': list(command), 'result': code,
            'before': before, 'after': after,
            'before_value': before_value, 'after_value': after_value,
            'requested': value, 'changed': after != before,
            'observed_value': after_value,
            'parameter_changed': after_value is not None and after_value != before_value,
            'graph_is_configured_description': True,
            'meaning': ('mpv reports the CONFIGURED filter description in `af`, which it does not '
                        'rewrite when a runtime command changes a parameter, so an unchanged '
                        'string is expected either way and proves nothing. This record establishes '
                        'callability and the before/after description only; proving the parameter '
                        'moved requires measuring the PCM output.')}


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


def repeat_pcm_fixture(source, destination, minimum_seconds=600.0):
    """Extend offline PCM bytes before playback; never loop/reload a live graph."""
    channels, width, rate, frames, raw = read_pcm_wave(source)
    if not frames:
        raise ValueError('runtime proof requires nonempty PCM input')
    repetitions = max(1, math.ceil(minimum_seconds * rate / frames))
    with wave.open(str(destination), 'wb') as output:
        output.setparams((channels, width, rate, 0, 'NONE', 'not compressed'))
        for _ in range(repetitions):
            output.writeframesraw(raw)
    return frames * repetitions


def runtime_pcm_capture(args, source, chain, command, expected_db):
    """Measure two sample windows in ONE uninterrupted native playback.

    PCM AO decodes much faster than wall time. A short input can reach EOF
    during wait_audio's first 50 ms pump; commands then cannot affect its
    already-written samples. Extend input bytes before loading, issue the
    command as soon as output exists, and analyse sample offsets after teardown.
    No pause, seek, filter replacement, or loadfile is issued across the change.
    """
    if not command:
        return {'error': 'unsupported af-command signature', 'runtime_confirmed': False}
    with tempfile.TemporaryDirectory(prefix='echo-runtime-pcm-') as workdir:
        extended, output = Path(workdir) / 'input.wav', Path(workdir) / 'output.wav'
        input_frames = repeat_pcm_fixture(source, extended)
        handle = Mpv(args.libmpv, ao='pcm', ao_pcm_file=output)
        try:
            if handle.command('af', 'set', chain) < 0:
                raise RuntimeError('runtime proof chain was rejected')
            if handle.command('loadfile', str(extended)) < 0:
                raise RuntimeError('runtime proof input was rejected')
            deadline = time.monotonic() + args.step_timeout
            params = None
            while time.monotonic() < deadline:
                handle.pump(0.001)
                params = handle.get('audio-out-params')
                if isinstance(params, dict) and output.exists():
                    # PCM AO currently writes signed 16-bit samples. Verify
                    # this assumption against the finalized WAVE below.
                    if output.stat().st_size >= params['samplerate'] * params['channel-count'] * 2:
                        break
            else:
                raise RuntimeError('runtime proof produced no baseline PCM')
            eof_before = handle.get('eof-reached')
            if eof_before:
                raise RuntimeError('runtime command would run after EOF; measurement refused')
            bytes_before = output.stat().st_size
            position_before = handle.get('time-pos')
            result = handle.command(*command)
            bytes_after = output.stat().st_size
            deadline = time.monotonic() + args.step_timeout
            minimum_bytes = bytes_after + params['samplerate'] * params['channel-count'] * 2 * 3
            while output.stat().st_size < minimum_bytes and not handle.get('eof-reached'):
                if time.monotonic() >= deadline:
                    raise RuntimeError('runtime proof capture did not reach EOF within timeout')
                handle.pump(0.001)
            operations = handle.operations
        finally:
            handle.close()
        channels, width, rate, frames, raw = read_pcm_wave(output)
        if width != 2 or channels != params['channel-count']:
            raise ValueError('PCM format changed; command offsets cannot be established')
        stride = channels * width
        # File byte counts include the header: these are conservative upper
        # bounds on frames already written, not exact audible command timing.
        command_frame_upper = math.ceil(bytes_after / stride)
        if frames - command_frame_upper < rate * 2:
            raise ValueError(f'no sufficiently long post-command PCM window: {frames=} {command_frame_upper=}')
        before_start, before_end = int(rate * 0.1), int(rate * 0.6)
        after_start, after_end = frames - rate, frames
        before = raw[before_start * stride:before_end * stride]
        after = raw[after_start * stride:after_end * stride]
        measured = measure_sine_gain_db(before, after, 997.0, channels, width, width, rate)
        return {'input': str(source), 'input_sha256': digest(source),
                'extended_input_sha256': digest(extended), 'extended_input_frames': input_frames,
                'output_sha256': digest(output), 'output_frames': frames,
                'sample_rate': rate, 'channels': channels, 'sample_width_bytes': width,
                'command': list(command), 'command_result': result, 'eof_before_command': eof_before,
                'time_pos_before_command': position_before,
                'file_bytes_before_command': bytes_before, 'file_bytes_after_command': bytes_after,
                'command_frame_upper_bound': command_frame_upper,
                'baseline_window_frames': [before_start, before_end],
                'changed_window_frames': [after_start, after_end],
                'measured_change_db': measured, 'expected_change_db': expected_db,
                'error_db': measured - expected_db,
                'runtime_confirmed': result >= 0 and abs(measured - expected_db) <= 0.25,
                'operations': operations,
                'meaning': 'One uninterrupted PCM capture; sample windows exclude initial and command transients. File offsets bound written samples, not real-time latency.'}


def spatial_width_capture(args, source, af_arg_count):
    return [runtime_pcm_capture(args, source, spatial_chain(),
            af_command_args(af_arg_count, 'echo_spatial', 'spatial', 'm', '1'),
            20.0 * math.log10(1.0 / 1.25))]


def static_gain_reference_db(args):
    """Measure a known install-time gain as a tone-analysis self-check.

    This is a static graph, so decoding to EOF is safe. The runtime verdict
    instead uses uninterrupted before/after PCM windows and an EOF guard.
    """
    input_channels, input_width, input_rate, _, input_pcm = read_pcm_wave(args.input)
    gain = float(args.capture_gain_db)
    chain = individual_eq_chain(input_rate, {5: gain}, args.capture_preamp_db)
    with tempfile.TemporaryDirectory() as workdir:
        wav = Path(workdir) / 'reference.wav'
        handle = Mpv(args.libmpv, ao='pcm', ao_pcm_file=wav)
        try:
            handle.command('af', 'set', chain)
            handle.command('loadfile', str(args.input.resolve()))
            wait_audio(handle, args.step_timeout)
            handle.set('pause', 'no')
            handle.pump(args.runtime_capture_seconds)
            handle.set('pause', 'yes')
        finally:
            handle.close()
        out_channels, out_width, out_rate, _, output_pcm = read_pcm_wave(wav)
    return measure_sine_gain_db(input_pcm, output_pcm, 997.0, input_channels, input_width,
                                out_width, out_rate)


def sweep_response_error_db(args, rate, analysis_rate):
    """Sweep-based magnitude-response check for the installed static chain.

    design.md section 3 promises "native low-level sweep / impulse response
    within 0.25 dB of the computed maximum error", but the recorded evidence only
    measured a 997 Hz tone against a single band.

    IMPORTANT — this reports a MEASUREMENT ENVELOPE, not the design tolerance.
    A windowed peak-envelope ratio over a logarithmic sweep cannot resolve
    0.25 dB: the window spans hundreds of Hz near 1 kHz, so the ratio tracks the
    window's frequency content rather than the filter's response. Measured
    behaviour during development: the error fell from +10.2 dB (10 ms window) to
    a constant +5.949 dB (80 ms window), i.e. it converges on the installed gain
    and the residual scatter is windowing artefact, not filter deviation.

    Resolving the promised tolerance needs a per-frequency sine sweep (one
    capture per tone, or an FFT with bin spacing narrower than the filter
    bandwidth), which is not implemented. The result is therefore reported as
    `usable_for_tolerance_verdict: false` so it cannot be mistaken for a pass.
    """
    sweep = sorted(args.input.parent.glob('*sweep*2ch*.wav'))
    same_rate = [path for path in sweep if read_pcm_wave(path)[2] == rate and '60db' in path.name]
    if not same_rate:
        return {'skipped': 'No low-level sweep fixture at this rate beside --input.'}
    source = same_rate[0]
    _, input_width, _, _, input_pcm = read_pcm_wave(source)
    chain = individual_eq_chain(rate, {5: args.capture_gain_db}, args.capture_preamp_db)
    with tempfile.TemporaryDirectory() as workdir:
        wav = Path(workdir) / 'sweep.wav'
        handle = Mpv(args.libmpv, ao='pcm', ao_pcm_file=wav)
        try:
            handle.command('af', 'set', chain)
            handle.command('loadfile', str(source))
            wait_audio(handle, args.step_timeout)
            handle.set('pause', 'no')
            handle.pump(args.runtime_capture_seconds)
            handle.set('pause', 'yes')
        finally:
            handle.close()
        _, out_width, _, _, output_pcm = read_pcm_wave(wav)

    # Scales are normalised per width: fixtures are 32-bit while ao=pcm emits
    # 16-bit, so comparing raw integers would report the format change as ~96 dB
    # of "response error".
    window = max(1, int(args.sweep_window_ms / 1000.0 * rate))
    in_scale = float(1 << (8 * input_width - 1))
    out_scale = float(1 << (8 * out_width - 1))
    errors = []
    limit = min(len(input_pcm) // (2 * input_width), len(output_pcm) // (2 * out_width))
    for block_start in range(window, limit - window, window):
        in_peak = 0.0
        out_peak = 0.0
        for index in range(block_start, block_start + window):
            in_peak = max(in_peak, abs(int.from_bytes(
                input_pcm[(index * 2) * input_width:(index * 2) * input_width + input_width],
                'little', signed=True)) / in_scale)
            out_peak = max(out_peak, abs(int.from_bytes(
                output_pcm[(index * 2) * out_width:(index * 2) * out_width + out_width],
                'little', signed=True)) / out_scale)
        if in_peak > 0 and out_peak > 0:
            errors.append(20.0 * math.log10(out_peak / in_peak))
    if not errors:
        return {'skipped': 'Sweep capture too short for windowed comparison.'}
    worst = max(errors, key=abs)
    installed = float(args.capture_gain_db)
    return {'input': source.name, 'windows': len(errors),
            'window_ms': args.sweep_window_ms,
            'mean_error_db': sum(errors) / len(errors),
            'max_abs_error_db': worst,
            'installed_gain_db': installed,
            'residual_after_removing_installed_gain_db': worst - installed,
            'converges_on_installed_gain': abs(worst - installed) < 1.0,
            'usable_for_tolerance_verdict': False,
            'tolerance_db_not_verified': args.response_tolerance_db,
            'chain': chain,
            'meaning': 'The envelope converges on the installed gain, so the chain is present and '
                       'scaled as designed. The residual scatter is windowing artefact and CANNOT '
                       'settle design section 3\'s 0.25 dB promise. Do not record this as a pass; '
                       'a per-frequency sine sweep or a sufficiently narrow FFT is still required.'}


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
        af_arg_count = af_command_signature(mpv)
        report['af_command_signature'] = {
            'argument_count': af_arg_count,
            'syntax': {3: 'patched-target-prefix', 4: 'upstream-separate-target'}.get(af_arg_count),
            'meaning': 'Read from the selected library command-list; unknown signatures are not guessed.',
        }
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
            # Runtime-parameter proof. mpv reports the CONFIGURED `af`
            # description and never rewrites it when a runtime command changes a
            # parameter, so the only admissible evidence is measured audio.
            if args.runtime_parameter_proof:
                for parameter, label, target, option, value, expected in (
                    ('equalizer', 'echo_eq5', 'eq5', 'gain', '6', 6.0),
                    ('preamp', 'echo_preamp', 'preamp', 'volume', '0.501187233627272', -6.0),
                ):
                    report['checks'].append({'name': 'runtime_parameter_pcm_proof', 'parameter': parameter,
                        **runtime_pcm_capture(args, args.input, individual_eq_chain(input_rate),
                            af_command_args(af_arg_count, label, target, option, value), expected)})
                # Width is observable only on a centre-free signal that does not
                # saturate the limiter, so pick a LOW-level antiphase fixture at
                # the input's own rate: with a centred or limiter-limited signal
                # both widths clip to the same ceiling and look identical.
                _, _, input_rate, _, _ = read_pcm_wave(args.input)
                candidates = sorted(args.input.parent.glob('*antiphase*2ch*.wav'))
                same_rate = [path for path in candidates if read_pcm_wave(path)[2] == input_rate]
                preferred = [path for path in same_rate if '60db' in path.name]
                antiphase = preferred or same_rate or candidates
                if antiphase:
                    report['checks'].append({'name': 'runtime_parameter_pcm_proof',
                                             'parameter': 'spatial_width', 'input': antiphase[0].name,
                                             'low_level': bool(preferred),
                                             **spatial_width_capture(args, antiphase[0], af_arg_count)[0]})
                else:
                    report['checks'].append({'name': 'runtime_parameter_pcm_proof',
                                             'skipped': 'No antiphase stereo fixture beside --input; '
                                                        'width changes are unobservable on a centred signal.'})
            if args.sweep_response:
                report['checks'].append({'name': 'sweep_response',
                                         **sweep_response_error_db(args, rate, rate)})
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
                # Values must differ from the installed ones (g=0, volume 0 dB)
                # or acceptance is indistinguishable from a silent no-op.
                commands = [('echo_eq', 'eq5', 'gain', '6'),
                            ('echo_eq', 'preamp', 'volume', '0.501187233627272')] if name == 'eq' else [
                            ('echo_eq5', 'eq5', 'gain', '6'),
                            ('echo_preamp', 'preamp', 'volume', '0.501187233627272')]
                for label, target, option, value in commands:
                    report['checks'].append({'name': 'af_command_parameter_change', **runtime_parameter_probe(
                        mpv, af_arg_count, label, target, option, value)})
            if name == 'spatial':
                # Installed m=1.25; 1.4 is the smallest change that a real
                # process_command must reflect in the configured graph.
                report['checks'].append({'name': 'spatial_parameter_change', **runtime_parameter_probe(
                    mpv, af_arg_count, 'echo_spatial', 'spatial', 'm', '1.4')})
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
            preamp_args = af_command_args(af_arg_count, 'echo_preamp', 'preamp', 'volume', '0.446683592150963')
            band_args = af_command_args(af_arg_count, 'echo_eq5', 'eq5', 'gain', '6')
            preamp_result = mpv.command(*preamp_args) if preamp_args else -1
            band_result = mpv.command(*band_args) if band_args else -1
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
                            'measurement': 'steady-state first-channel sinusoidal projection, 100 ms transient excluded; expected net includes installed EQ and preamp' ,
                        }
            except (EOFError, ValueError, struct.error) as error:
                report['capture']['analysis_error'] = f'{type(error).__name__}: {error}'

            # Harness self-check. The runtime-parameter verdict below is only
            # meaningful if this harness can detect a KNOWN static gain, so
            # record the install-time reference alongside it. A harness that
            # reports 0 dB for everything would otherwise "confirm" that
            # runtime commands do nothing.
            if args.runtime_parameter_proof:
                reference = static_gain_reference_db(args)
                if reference is None:
                    report['checks'].append({
                        'name': 'static_gain_reference',
                        'error': 'could not measure the install-time reference gain; the runtime '
                                 'verdict below is NOT trustworthy without it',
                    })
                else:
                    report['checks'].append({
                        'name': 'static_gain_reference',
                        'expected_db': float(args.capture_gain_db) + float(args.capture_preamp_db),
                        'measured_db': reference,
                        'error_db': reference - float(args.capture_gain_db) - float(args.capture_preamp_db),
                        'harness_valid': abs(reference - float(args.capture_gain_db) - float(args.capture_preamp_db)) <= 0.25,
                        'meaning': 'Install-time gain measured through the same harness. If this is '
                                   'not within 0.25 dB, a negative runtime result means the harness '
                                   'is broken, not that the command was ignored.',
                    })


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
    parser.add_argument('--runtime-parameter-proof', action='store_true',
                        help='With --capture-only, measure EQ, preamp and spatial bypass commands '
                             'during uninterrupted PCM playback; extends fixture bytes offline.')
    parser.add_argument('--runtime-capture-seconds', type=float, default=4.0,
                        help='Wall-clock pump duration for static captures; runtime parameter proofs '
                             'use PCM sample windows and do not infer captured duration from wall time.')
    parser.add_argument('--sweep-response', action='store_true',
                        help='With --capture-only, also measure the installed chain response across '
                             'the logarithmic sweep fixture and compare it against the design tolerance.')
    parser.add_argument('--response-tolerance-db', type=float, default=0.25,
                        help='The tolerance design.md section 3 promises. RECORDED ONLY: the '
                             'windowed-sweep method cannot resolve it, so the check reports '
                             'usable_for_tolerance_verdict=false instead of a pass.')
    parser.add_argument('--sweep-window-ms', type=float, default=80.0,
                        help='Envelope window for --sweep-response. A logarithmic sweep needs a '
                             'window wide enough that one window spans a small fraction of the '
                             'octave; too narrow and the ratio tracks window frequency content '
                             'instead of the filter response.')
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
    if args.runtime_parameter_proof and not args.capture_only:
        parser.error('--runtime-parameter-proof requires --capture-only')
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
