#!/usr/bin/env python3
"""macOS-only native probe for the loudness-normalization gain stage.

This measures the exact ``volume`` node that ``native_effects.rs`` drives with
``10 ** (dB / 20)``. It is deliberately narrower than the three-platform release
Gate: it proves the packaged macOS library changes PCM when that command is
issued, that the gain-only chain has no EQ/spatial node, and that the terminal
limiter keeps a boosted near-full-scale signal at or below 0 dBFS.

It does NOT establish 30 ms transition smoothness, real-time latency, device
underruns, float-chain non-finite values, Windows/Linux behaviour, BS.1770
loudness or true-peak compliance (``ao=pcm`` writes integer PCM16 and is
untimed). Every report keeps ``gate_passed=false``.
"""
import argparse
import json
import math
import platform
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import probe as base  # noqa: E402  (reuse the pinned native-client helpers)
from mpv_client import Mpv  # noqa: E402

CHAIN_NAME = "echo_normalization_gain_only"
LIMITER = base.LIMITER


def gain_only_chain(gain_db=0.0):
    """The production gain-only chain, with an optional install-time gain."""
    volume = 10 ** (gain_db / 20.0)
    return (
        "@echo_float:lavfi=[aformat=sample_fmts=dblp],"
        f"@echo_preamp:lavfi=[volume@preamp=volume={volume:.15g}:precision=double],"
        f"@echo_limiter:lavfi=[{LIMITER}]"
    )


def volume_command(argument_count, value):
    """The exact af-command syntax used by ``preamp()`` on this library."""
    return base.af_command_args(
        argument_count, "echo_preamp", "preamp", "volume", f"{value:.15g}"
    )


def chain_shape(libmpv):
    """Structural: three nodes, no equalizer / extrastereo, limiter last."""
    handle = Mpv(libmpv)
    try:
        if handle.command("af", "set", gain_only_chain()) < 0:
            raise RuntimeError("gain-only chain was rejected")
        handle.pump(0.1)
        graphs = base.graph_text(handle)
        entries = handle.get("af")
    finally:
        handle.close()
    labels = [
        entry.get("label") or entry.get("name")
        for entry in entries
        if isinstance(entry, dict)
    ] if isinstance(entries, list) else []
    return {
        "graph": graphs,
        "labels": labels,
        "node_count": len(entries) if isinstance(entries, list) else 0,
        "has_equalizer": "equalizer" in (graphs or ""),
        "has_extrastereo": "extrastereo" in (graphs or ""),
        "limiter_last": "alimiter" in (graphs or "").split(",")[-1],
        "no_eq_or_spatial_node": "equalizer" not in (graphs or "")
        and "extrastereo" not in (graphs or ""),
    }


def static_gain_reference(libmpv, source, gain_db, step_timeout, capture_seconds):
    """Harness self-check: detect a KNOWN install-time gain through PCM."""
    channels, width, rate, _, input_pcm = base.read_pcm_wave(source)
    with tempfile.TemporaryDirectory(prefix="echo-norm-reference-") as workdir:
        wav = Path(workdir) / "reference.wav"
        handle = Mpv(libmpv, ao="pcm", ao_pcm_file=wav)
        try:
            if handle.command("af", "set", gain_only_chain(gain_db)) < 0:
                raise RuntimeError("reference chain was rejected")
            handle.command("loadfile", str(source.resolve()))
            if not base.wait_audio(handle, step_timeout):
                raise RuntimeError("reference capture produced no audio")
            handle.set("pause", "no")
            handle.pump(capture_seconds)
            handle.set("pause", "yes")
        finally:
            handle.close()
        out_channels, out_width, out_rate, _, output_pcm = base.read_pcm_wave(wav)
    measured = base.measure_sine_gain_db(
        input_pcm, output_pcm, 997.0, channels, width, out_width, out_rate
    )
    return {
        "expected_db": gain_db,
        "measured_db": measured,
        "error_db": measured - gain_db,
        "harness_valid": abs(measured - gain_db) <= 0.25,
        "meaning": "Install-time gain measured through the same PCM harness; a harness that reads 0 dB for a known gain is broken.",
    }


def pressure_capture(libmpv, source, gain_db, step_timeout, capture_seconds):
    """Integer-PCM peak after a positive gain, with the limiter in the chain."""
    channels, width, rate, frames, _ = base.read_pcm_wave(source)
    with tempfile.TemporaryDirectory(prefix="echo-norm-pressure-") as workdir:
        wav = Path(workdir) / "pressure.wav"
        handle = Mpv(libmpv, ao="pcm", ao_pcm_file=wav)
        try:
            if handle.command("af", "set", gain_only_chain(gain_db)) < 0:
                raise RuntimeError("pressure chain was rejected")
            handle.command("loadfile", str(source.resolve()))
            if not base.wait_audio(handle, step_timeout):
                raise RuntimeError("pressure capture produced no audio")
            handle.set("pause", "no")
            handle.pump(capture_seconds)
            handle.set("pause", "yes")
        finally:
            handle.close()
        out_channels, out_width, out_rate, out_frames, raw = base.read_pcm_wave(wav)
    scale = float(1 << (out_width * 8 - 1))
    peak = 0
    for offset in range(0, len(raw), out_channels * out_width):
        sample = int.from_bytes(raw[offset : offset + out_width], "little", signed=True)
        peak = max(peak, abs(sample))
    peak_linear = peak / scale
    return {
        "input": source.name,
        "input_sha256": base.digest(source),
        "gain_db": gain_db,
        "output_frames": out_frames,
        "sample_rate": out_rate,
        "channels": out_channels,
        "sample_width_bytes": out_width,
        "peak_linear": peak_linear,
        "peak_dbfs": 20.0 * math.log10(peak_linear) if peak_linear > 0 else None,
        "at_or_below_full_scale": peak_linear <= 1.0,
        "meaning": "Integer PCM16 peak with the terminal limiter active. It cannot prove the intermediate float chain had no non-finite values, and is not a true-peak measurement.",
    }


def build_report(args, argument_count):
    report = {
        "probe": CHAIN_NAME,
        "platform": platform.platform(),
        "platform_verified": "macos",
        "other_platforms": "deferred; no Windows/Linux real-machine capture in this run",
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "libmpv": str(args.libmpv),
        "libmpv_sha256": base.digest(args.libmpv),
        "manifest": str(args.manifest) if args.manifest else None,
        "manifest_sha256": base.digest(args.manifest) if args.manifest else None,
        "fixture_manifest_sha256": base.digest(args.fixture_manifest),
        "input": str(args.input),
        "input_sha256": base.digest(args.input),
        "pressure_input": str(args.pressure_input),
        "pressure_input_sha256": base.digest(args.pressure_input),
        "af_command_argument_count": argument_count,
        "chain": gain_only_chain(),
        "gate_passed": False,
        "gate_scope": "macOS PCM capture only; three-platform release Gate remains open",
        "checks": [],
    }
    shape = chain_shape(args.libmpv)
    report["checks"].append({"name": "gain_only_chain_shape", **shape})
    report["checks"].append(
        {
            "name": "static_gain_reference",
            **static_gain_reference(
                args.libmpv, args.input, 6.0, args.step_timeout, args.capture_seconds
            ),
        }
    )
    runtime_args = argparse.Namespace(
        libmpv=args.libmpv,
        step_timeout=args.step_timeout,
        runtime_capture_seconds=args.capture_seconds,
    )
    # baseline 0 dB -> -6 dB, baseline 0 dB -> +6 dB, +6 dB -> 0 dB (disable).
    for label, install_db, target_db, expected_db in (
        ("normalization_minus_6_db", 0.0, -6.0, -6.0),
        ("normalization_plus_6_db", 0.0, 6.0, 6.0),
        ("normalization_disable_returns_to_zero", 6.0, 0.0, -6.0),
    ):
        command = volume_command(argument_count, 10 ** (target_db / 20.0))
        result = base.runtime_pcm_capture(
            runtime_args,
            args.input,
            gain_only_chain(install_db),
            command,
            expected_db,
        )
        report["checks"].append(
            {"name": "runtime_gain_pcm_proof", "case": label, **result}
        )
    report["checks"].append(
        {
            "name": "pressure_baseline",
            **pressure_capture(
                args.libmpv, args.pressure_input, 0.0, args.step_timeout, args.capture_seconds
            ),
        }
    )
    report["checks"].append(
        {
            "name": "pressure_plus_6_db",
            **pressure_capture(
                args.libmpv, args.pressure_input, 6.0, args.step_timeout, args.capture_seconds
            ),
        }
    )
    report["not_measured"] = [
        "30 ms track-to-track gain ramp smoothness (PCM AO is untimed)",
        "real-time update latency, underruns and device rebuilds",
        "intermediate float-chain non-finite diagnostics and true-peak",
        "Windows and Linux distributed runtimes",
        "BS.1770 loudness measurement or true-peak compliance",
    ]
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--libmpv", type=Path, required=True)
    parser.add_argument("--input", type=Path, required=True, help="Low-level 997 Hz tone.")
    parser.add_argument("--pressure-input", type=Path, required=True, help="Near-full-scale tone.")
    parser.add_argument("--fixture-manifest", type=Path, required=True)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--step-timeout", type=float, default=10.0)
    parser.add_argument("--capture-seconds", type=float, default=2.0)
    args = parser.parse_args()
    for path in (args.libmpv, args.input, args.pressure_input, args.fixture_manifest):
        if not path.is_file():
            parser.error(f"missing input: {path}")
    handle = Mpv(args.libmpv)
    try:
        argument_count = base.af_command_signature(handle)
    finally:
        handle.close()
    report = {"status": "probe-only", "gate_passed": False}
    try:
        report.update(build_report(args, argument_count))
    except Exception as error:  # noqa: BLE001 - native failure must still write a report
        report["status"] = "failed"
        report["error"] = f"{type(error).__name__}: {error}"
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{report['status']}: {args.output}; gate_passed=false")
    return 0 if report["status"] == "probe-only" else 1


if __name__ == "__main__":
    sys.exit(main())
