"""Numerical and evidence-rejection tests; these are not native acceptance."""
import math
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import json
import subprocess
import sys
import wave

import sine_sweep as sweep


class SteppedSineTests(unittest.TestCase):
    def test_grid_covers_centers_neighbors_and_low_rate_exclusion(self):
        low = sweep.frequencies(22050)
        self.assertGreaterEqual(len(low), 120)
        self.assertIn(8000, low)
        self.assertNotIn(16000, low)
        self.assertAlmostEqual(max(low), 0.499 * 22050)
        self.assertIn(20000, sweep.frequencies(48000))
        with self.assertRaises(ValueError):
            sweep.frequencies(48000, 119)

    def test_curve_composes_bands_and_skips_invalid_band(self):
        gains = [0] * 10
        gains[5] = 6
        self.assertAlmostEqual(sweep.curve_response_db(gains, 48000, 1000), 6, places=10)
        gains[6] = 6
        self.assertGreater(sweep.curve_response_db(gains, 48000, 1000), 6.5)
        gains = [0] * 9 + [12]
        self.assertAlmostEqual(sweep.curve_response_db(gains, 22050, 8000, -2), -2)
        self.assertGreater(sweep.curve_response_db(gains, 48000, 8000), 0)

    def test_fit_handles_noninteger_cycles_phase_and_dc(self):
        rate, frequency, start, end = 48000, 31.25, 14000, 64000
        amplitude = 0.001
        raw = bytearray(end * 8)
        for i in range(end):
            value = round((amplitude * math.sin(math.tau * frequency * i / rate + 0.73) + 0.00002) * 2 ** 31)
            struct.pack_into('<ii', raw, i * 8, value, value)
        result = sweep.project_s32(raw, 2, rate, frequency, start, end)
        self.assertAlmostEqual(result['amplitude'], amplitude, places=9)
        self.assertAlmostEqual(result['dc'], 0.00002, places=9)
        self.assertLess(result['quantization_bound_db'], 0.01)

    def test_rejects_quantized_or_limited_window(self):
        rate, frames = 48000, 4800
        for level, message in ((1e-8, 'quantization'), (0.5, 'limiter')):
            raw = bytearray(frames * 8)
            for i in range(frames):
                value = round(level * math.sin(math.tau * 1000 * i / rate) * 2 ** 31)
                struct.pack_into('<ii', raw, i * 8, value, value)
            with self.assertRaisesRegex(ValueError, message):
                sweep.project_s32(raw, 2, rate, 1000, 0, frames)
        with self.assertRaisesRegex(ValueError, '32 cycles'):
            sweep.project_s32(raw, 2, rate, 1000, 0, 100)

    def test_rejects_s16_and_frame_or_rate_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'capture.wav'
            for width, rate, frames in ((2, 48000, 2), (4, 44100, 2), (4, 48000, 1)):
                with wave.open(str(path), 'wb') as target:
                    target.setparams((2, width, rate, 0, 'NONE', 'not compressed'))
                    target.writeframes(bytes(width * 2 * frames))
                with self.assertRaisesRegex(ValueError, 'exact frames'):
                    sweep.validate_capture(path, 48000, 2)

    def test_rejects_extensible_24_valid_bits_in_s32_container(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'capture.wav'
            guid = bytes.fromhex('0100000000001000800000aa00389b71')
            fmt = struct.pack('<HHIIHHHHI', 0xFFFE, 2, 48000, 384000, 8, 32, 22, 24, 3) + guid
            chunks = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', 16) + bytes(16)
            path.write_bytes(b'RIFF' + struct.pack('<I', len(chunks) + 4) + b'WAVE' + chunks)
            with self.assertRaisesRegex(ValueError, '32 valid PCM bits'):
                sweep.validate_capture(path, 48000, 2)

    def test_fixture_settles_low_frequencies_for_24_cycles(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'tone.wav'
            windows, frames = sweep.generate_tones(path, 22050, [20, 1000])
            self.assertGreaterEqual(windows[0]['settle_frames'] * 20 / 22050, 24)
            self.assertGreaterEqual((windows[0]['window_frames'][1] - windows[0]['window_frames'][0]) * 20 / 22050, 32)
            self.assertEqual(len(sweep.validate_capture(path, 22050, frames)), frames * 8)

    def test_rejects_harmonic_contamination(self):
        rate, frames = 48000, 4800
        raw = bytearray(frames * 8)
        for i in range(frames):
            phase = math.tau * 1000 * i / rate
            value = round((0.001 * math.sin(phase) + 0.00001 * math.sin(phase * 2)) * 2 ** 31)
            struct.pack_into('<ii', raw, i * 8, value, value)
        with self.assertRaisesRegex(ValueError, 'residual'):
            sweep.project_s32(raw, 2, rate, 1000, 0, frames)

    def test_native_timeout_does_not_leave_stale_success(self):
        with tempfile.TemporaryDirectory() as directory:
            library, output = Path(directory) / 'library', Path(directory) / 'report.json'
            library.write_bytes(b'explicit library path')
            output.write_text('{"response_passed":true}')
            argv = ['sine_sweep.py', '--libmpv', str(library), '--rate', '48000', '--output', str(output)]
            with patch.object(sys, 'argv', argv), patch.object(sweep, 'provenance', return_value={'test': True}), patch.object(sweep.subprocess, 'run', side_effect=subprocess.TimeoutExpired('worker', 240)):
                self.assertEqual(sweep.main(), 1)
            report = json.loads(output.read_text())
            self.assertEqual(report['status'], 'timeout')
            self.assertFalse(report['response_passed'])
            self.assertFalse(report['gate_passed'])


if __name__ == '__main__':
    unittest.main()
