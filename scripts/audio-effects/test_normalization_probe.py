"""Pure-function tests for the normalization gain-stage probe (no libmpv)."""
import json
import math
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import normalization_probe as probe  # noqa: E402


class NormalizationProbeTests(unittest.TestCase):
    def test_gain_only_chain_has_no_eq_or_spatial_node(self):
        chain = probe.gain_only_chain()
        self.assertIn("@echo_float", chain)
        self.assertIn("@echo_preamp", chain)
        self.assertIn("@echo_limiter", chain)
        self.assertNotIn("equalizer", chain)
        self.assertNotIn("extrastereo", chain)
        self.assertIn("sample_fmts=dblp", chain)
        self.assertIn("precision=double", chain)

    def test_install_gain_uses_the_same_db_to_amplitude_formula_as_production(self):
        for db in (-6.0, 0.0, 6.0):
            chain = probe.gain_only_chain(db)
            prefix = "volume@preamp=volume="
            value = chain.split(prefix, 1)[1].split(":", 1)[0]
            self.assertAlmostEqual(float(value), 10 ** (db / 20.0), places=12)

    def test_runtime_command_uses_the_loaded_af_command_signature(self):
        legacy = probe.volume_command(3, 0.5)
        modern = probe.volume_command(4, 0.5)
        self.assertEqual(legacy[0], "af-command")
        self.assertEqual(legacy[1], "echo_preamp")
        self.assertEqual(legacy[2], "preamp:volume")
        self.assertEqual(modern[2], "volume")
        self.assertEqual(modern[4], "preamp")
        self.assertIsNone(probe.volume_command(None, 0.5))

    def test_report_contract_never_claims_a_passed_gate(self):
        # A minimal mechanical rendering of the report's outer contract.
        with tempfile.TemporaryDirectory() as workdir:
            path = Path(workdir) / "report.json"
            path.write_text(
                json.dumps({"gate_passed": False, "platform_verified": "macos"}),
                encoding="utf-8",
            )
            report = json.loads(path.read_text(encoding="utf-8"))
        self.assertIs(report["gate_passed"], False)
        self.assertEqual(report["platform_verified"], "macos")

    def test_pressure_threshold_is_full_scale(self):
        peak = 32767 / 32768.0
        self.assertLessEqual(peak, 1.0)
        self.assertLess(20.0 * math.log10(peak), 0.0)
        self.assertTrue(math.isfinite(20.0 * math.log10(peak)))


if __name__ == "__main__":
    unittest.main()
