"""Tool correctness tests, deliberately separate from native audio acceptance."""
import ctypes as C
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import wave

from generate_fixtures import generate, samples
from mpv_client import Node, NodeList, decode, node_value
import probe


class FixtureTests(unittest.TestCase):
    def test_matrix_content_peaks_and_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = generate(root, 0.05)
            self.assertEqual(len(manifest['files']), 80)
            self.assertEqual({item['rate_hz'] for item in manifest['files']}, {22050, 44100, 48000, 96000})
            for item in manifest['files']:
                self.assertEqual(item['license'], 'CC0-1.0')
                path = root / item['file']
                self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), item['sha256'])
                with wave.open(str(path)) as source:
                    self.assertEqual(source.getnframes(), item['frames'])
                    self.assertEqual(source.getnchannels(), item['channels'])
                    raw = source.readframes(source.getnframes())
                values = struct.unpack('<' + 'i' * (len(raw) // 4), raw)
                measured = 20 * math.log10(max(abs(v) for v in values) / 2147483648)
                self.assertAlmostEqual(measured, item['requested_sample_peak_dbfs'], places=5)
                if item['kind'] == 'left-only':
                    self.assertTrue(all(v == 0 for v in values[1::2]))
                if item['kind'] == 'antiphase':
                    self.assertTrue(all(left == -right for left, right in zip(values[0::2], values[1::2])))
                if item['kind'] == 'impulse':
                    self.assertEqual(sum(v != 0 for v in values), item['channels'])

    def test_byte_reproducibility(self):
        with tempfile.TemporaryDirectory() as directory:
            a = generate(Path(directory) / 'a', 0.05, [48000])
            b = generate(Path(directory) / 'b', 0.05, [48000])
            self.assertEqual(a, b)

    def test_invalid_duration_and_rate(self):
        with tempfile.TemporaryDirectory() as directory:
            for duration in (0, -1, math.inf, math.nan, 61):
                with self.assertRaises(ValueError):
                    generate(Path(directory), duration)
            with self.assertRaises(ValueError):
                generate(Path(directory), rates=[12345])

    def test_noise_algorithm_is_pinned(self):
        values = list(samples('noise', 48000, 4))
        self.assertEqual(values, list(samples('noise', 96000, 4)))
        self.assertAlmostEqual(values[0], 0.2899612386920398)


class ProbeTests(unittest.TestCase):
    def test_af_command_uses_only_the_detected_library_signature(self):
        command_list = [
            {'name': 'set', 'args': ['property', 'value']},
            {'name': 'af-command', 'args': ['label', 'command', 'argument']},
        ]
        class MpvStub:
            def get(self, _):
                return command_list

        mpv = MpvStub()
        self.assertEqual(probe.af_command_signature(mpv), 3)
        self.assertEqual(
            probe.af_command_args(3, 'echo_eq5', 'eq5', 'gain', '6'),
            ('af-command', 'echo_eq5', 'eq5:gain', '6'),
        )
        command_list[1]['args'].append('target')
        self.assertEqual(probe.af_command_signature(mpv), 4)
        self.assertEqual(
            probe.af_command_args(4, 'echo_eq5', 'eq5', 'gain', '6'),
            ('af-command', 'echo_eq5', 'gain', '6', 'eq5'),
        )
        self.assertIsNone(probe.af_command_args(None, 'echo_eq5', 'eq5', 'gain', '6'))

    def test_candidate_inactive_bands_and_safety_options(self):
        chain = probe.eq_chain(22050)
        self.assertEqual(chain.count('equalizer@'), 9)
        self.assertNotIn('equalizer@eq9', chain)
        self.assertEqual(probe.eq_chain(48000).count('equalizer@'), 10)
        self.assertIn('precision=f64', chain)
        self.assertNotIn('block_size=', chain)
        self.assertIn('asc=false:level=false:latency=true', chain)
        self.assertIn('extrastereo@spatial=m=1.25:c=false', probe.spatial_chain())
        self.assertIn('equalizer@eq5=f=1000:t=q:w=', probe.individual_eq_chain(48000, {5: 6.0}))
        self.assertIn(':g=6:mix=1', probe.individual_eq_chain(48000, {5: 6.0}))
        self.assertIn('volume@preamp=volume=0.266072505979881',
                      probe.individual_eq_chain(48000, {5: 12.0}, -11.5))

    def test_node_map_decoding_copies_values(self):
        values = (Node * 2)()
        values[0].format, values[0].u.int64 = 4, 48000
        values[1].format, values[1].u.string = 1, b'stereo'
        keys = (C.c_char_p * 2)(b'samplerate', b'channels')
        listing = NodeList(2, values, keys)
        node = Node()
        node.format, node.u.list = 8, C.pointer(listing)
        result = node_value(node)
        values[0].u.int64 = 44100
        self.assertEqual(result, {'samplerate': 48000, 'channels': 'stereo'})

    def test_pcm_wave_reader_accepts_extensible_integer_pcm(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'extensible.wav'
            pcm_guid = bytes.fromhex('0100000000001000800000aa00389b71')
            fmt = struct.pack('<HHIIHHHHI', 0xFFFE, 2, 48000, 384000, 8, 32, 22, 32, 3) + pcm_guid
            raw = struct.pack('<iiii', 0, 2147483647, -2147483648, 0)
            chunks = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(raw)) + raw
            path.write_bytes(b'RIFF' + struct.pack('<I', len(chunks) + 4) + b'WAVE' + chunks)

            channels, width, rate, frames, decoded = probe.read_pcm_wave(path)

        self.assertEqual((channels, width, rate, frames), (2, 4, 48000, 2))
        self.assertEqual(decoded, raw)

    def test_sine_projection_reports_db_gain(self):
        rate, channels, width, frames = 48000, 2, 2, 48000
        source, output = bytearray(), bytearray()
        for frame in range(frames):
            sample = int(10000 * math.sin(math.tau * 997 * frame / rate))
            raised = int(sample * 10 ** (6 / 20))
            source.extend(struct.pack('<hh', sample, sample))
            output.extend(struct.pack('<hh', raised, raised))
        measured = probe.measure_sine_gain_db(source, output, 997, channels, width, width, rate)
        self.assertAlmostEqual(measured, 6.0, places=3)

    def test_loading_failure_preserves_selected_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'distributed.so'
            path.write_bytes(b'explicit package fixture')
            args = type('Args', (), {'libmpv': path, 'input': path, 'ao': 'null', 'pcm_output': None, 'package': None, 'manifest': None})()
            with patch.object(probe, 'Mpv', side_effect=OSError('missing native symbol')) as constructor:
                result = probe.run_probe(args)
            constructor.assert_called_once_with(path.resolve(), 'null', None)
            self.assertEqual(result['status'], 'failed')
            self.assertFalse(result['gate_passed'])
            self.assertEqual(result['libmpv']['sha256'], probe.digest(path))
            self.assertFalse(result['capture']['performed'])

    def test_native_worker_timeout_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'artifact'
            path.write_bytes(b'explicit')
            output = Path(directory) / 'report.json'
            argv = ['probe.py', '--libmpv', str(path), '--input', str(path), '--output=' + str(output), '--timeout', '0.1']
            with patch.object(sys, 'argv', argv), patch.object(probe.subprocess, 'run', side_effect=subprocess.TimeoutExpired('worker', 0.1)):
                self.assertEqual(probe.main(), 1)
            report = json.loads(output.read_text())
            self.assertEqual(report['status'], 'timeout')
            self.assertFalse(report['gate_passed'])


if __name__ == '__main__':
    unittest.main()
