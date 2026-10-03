#!/usr/bin/env python3
"""Report which audio filters accept a RUNTIME command, from real dylibs.

`avfilter_graph_send_command` returns AVERROR(ENOSYS) when the target filter has
no `process_command` callback. mpv's `f_lavfi.c` only tests `result >= 0`, so a
rejected command is reported to the caller as SUCCESS. Return codes from
`af-command` are therefore not evidence that a parameter changed.

This reads the `process_command` field of each `AVFilter` struct directly, which
needs no audio and no graph. Three traps it avoids:

1. FFmpeg 6.0 declares the public fields in this order:
   name, description, inputs, outputs, priv_class, flags,
   nb_inputs, nb_outputs, formats_state, preinit, init, uninit, formats,
   priv_size, flags_internal, process_command, activate
   Note that `inputs`/`outputs` come BEFORE `priv_class`, `nb_inputs`/`nb_outputs`
   are `uint8_t` packed together after `flags`, and `priv_size` only appears at
   offset 80. A 2026-10-03 revision used the order
   `name, description, priv_size, flags_internal, priv_class, inputs, ...`
   instead, which misread every field after `description`: it reported
   `nb_inputs=65536` and produced a false "extrastereo has no process_command"
   that became the basis for the "width is install-time only" conclusion.
   Upstream FFmpeg contradicts that conclusion: `af_extrastereo.c` assigns
   `.process_command = ff_filter_process_command`.
2. Comparing `name` proves nothing about later offsets, because `name` sits at
   offset 0 and reads correctly under ANY layout. This tool therefore
   cross-checks `nb_inputs`/`nb_outputs` against the exported
   `avfilter_filter_pad_count()` and requires `priv_size` to be a plausible
   struct size, then REFUSES to print per-filter verdicts if either fails.
3. `av_buffersink_add_frame` and `av_buffersrc_add_frame` are inline in
   libavfilter and not resolvable through dlsym, so output-level measurement
   still needs a different harness. Structural agreement also does not prove a
   command takes effect: confirm with a command whose value differs from the
   installed one, then re-read the `af` graph.

Usage:
    python3 scripts/audio-effects/inspect-filter-runtime.py \
        --vendor apps/desktop/src-tauri/vendor/libmpv/macos \
        [--library /Applications/Echo.app/Contents/Frameworks/libavfilter.dylib]
"""
from __future__ import annotations

import argparse
import ctypes as C
import hashlib
import sys
from pathlib import Path

# Field order per FFmpeg 6.0 libavfilter/avfilter.h. The order below is the
# one that header actually declares; note that `inputs`/`outputs` precede
# `priv_class`, the pad counts are uint8_t packed after `flags`, and
# `priv_size` sits at offset 80 rather than next to `description`.
FIELDS_60 = [
    "name", "description", "inputs", "outputs", "priv_class", "flags",
    "nb_inputs", "nb_outputs", "formats_state", "preinit", "init", "uninit",
    "formats", "priv_size", "flags_internal", "process_command", "activate",
]

# Layout constraints that a correct reading must satisfy. `name` cannot
# establish the layout because it lives at offset 0, so the pad counts are
# cross-checked against the exported avfilter_filter_pad_count() instead of
# being assumed. These filters all declare exactly one input and one output.
MAX_PLAUSIBLE_PRIV_SIZE = 1 << 16

DEFAULT_FILTERS = ["equalizer", "volume", "alimiter", "extrastereo", "aformat"]


class AVOption(C.Structure):
    _fields_ = [
        ("name", C.c_char_p), ("help", C.c_char_p),
        ("offset", C.c_int64), ("type", C.c_int),
        ("min", C.c_double), ("max", C.c_double), ("flags", C.c_int),
        ("unit", C.c_char_p),
    ]


class AVClass(C.Structure):
    pass


AVClass._fields_ = [
    ("class_name", C.c_char_p), ("class_version", C.c_int),
    ("item_name", C.c_void_p), ("option", C.POINTER(AVOption)),
    ("version", C.c_uint), ("log_level_offset", C.c_int),
    ("parent_log_context_offset", C.c_int), ("parent_class", C.POINTER(AVClass)),
    ("category", C.c_uint), ("priv_class", C.POINTER(AVClass)),
    ("item_name_prefix", C.c_void_p), ("flags", C.c_int),
]


class AVFilterPad(C.Structure):
    _fields_ = [
        ("name", C.c_char_p), ("type", C.c_int), ("filter", C.c_void_p),
        ("filter_ctx", C.c_void_p), ("pad_idx", C.c_int),
        ("start_frame", C.c_void_p), ("uninit_frame", C.c_void_p),
        ("process_frame", C.c_void_p), ("filter_frame", C.c_void_p),
        ("request_frame", C.c_void_p), ("poll_frame", C.c_void_p),
        ("activate_func", C.c_void_p), ("filter_opt", C.POINTER(AVOption)),
    ]


class AVFilter(C.Structure):
    # Mirrors FFmpeg 6.0 libavfilter/avfilter.h exactly. The previous order put
    # priv_size/flags_internal/priv_class before inputs/outputs and used
    # 32-bit pad counts, which shifted every field after `description`.
    _fields_ = [
        ("name", C.c_char_p), ("description", C.c_char_p),
        ("inputs", C.POINTER(AVFilterPad)), ("outputs", C.POINTER(AVFilterPad)),
        ("priv_class", C.POINTER(AVClass)), ("flags", C.c_int),
        ("nb_inputs", C.c_uint8), ("nb_outputs", C.c_uint8),
        ("formats_state", C.c_uint8),
        ("preinit", C.c_void_p), ("init", C.c_void_p), ("uninit", C.c_void_p),
        ("formats", C.c_void_p),
        ("priv_size", C.c_int), ("flags_internal", C.c_int),
        ("process_command", C.c_void_p),
        ("activate", C.c_void_p),
    ]


def digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def load(path: Path):
    lib = C.CDLL(str(path))
    get_by_name = lib.avfilter_get_by_name
    get_by_name.restype = C.c_void_p
    get_by_name.argtypes = [C.c_char_p]
    # Exported since FFmpeg 4.0; gives pad counts without trusting our layout.
    pad_count = lib.avfilter_filter_pad_count
    pad_count.restype = C.c_uint
    pad_count.argtypes = [C.c_void_p, C.c_uint]
    try:
        lib.avutil_version.restype = C.c_uint
        lib.avutil_version.argtypes = []
    except AttributeError:
        pass
    return lib, get_by_name, pad_count


def inspect(library: Path, names: list[str]) -> int:
    lib, get_by_name, pad_count = load(library)
    print(f"library : {library}")
    print(f"sha256  : {digest(library)}")
    print()
    rows = []
    for name in names:
        handle = get_by_name(name.encode())
        if not handle:
            print(f"{name:<14} {'no':<8} {'-':<16} not compiled into this build")
            continue
        filt = C.cast(handle, C.POINTER(AVFilter)).contents
        decoded = filt.name.decode() if filt.name else ""
        rows.append((name, decoded, int(filt.priv_size), int(filt.nb_inputs), int(filt.nb_outputs),
                     bool(filt.process_command), pad_count(handle, 0), pad_count(handle, 1)))

    # Layout assertions. A misaligned struct still yields a readable `name`, so
    # the pad counts are compared against the library's own answers instead.
    problems = []
    for name, decoded, priv_size, nb_in, nb_out, _, auth_in, auth_out in rows:
        if decoded != name:
            problems.append(f"{name}: read name {decoded!r}, expected {name!r}")
        if nb_in != auth_in or nb_out != auth_out:
            problems.append(f"{name}: read nb_inputs/nb_outputs {nb_in}/{nb_out}, "
                            f"library reports {auth_in}/{auth_out}")
        if not 0 < priv_size <= MAX_PLAUSIBLE_PRIV_SIZE:
            problems.append(f"{name}: priv_size {priv_size} is not a plausible struct size "
                            f"(expected 1..{MAX_PLAUSIBLE_PRIV_SIZE})")

    if problems:
        print("struct layout FAILED its own assertions, so no per-filter verdict is reported:")
        for problem in problems:
            print(f"  - {problem}")
        print("\nThe process_command column is meaningless until these pass. Check the field")
        print("order and widths against this libavfilter's avfilter.h.")
        return 2

    print(f"{'filter':<14} {'priv_size':<11} {'pads':<7} {'process_command':<16} verdict")
    for name, _, priv_size, nb_in, nb_out, callback, _, _ in rows:
        verdict = "runtime command accepted" if callback else "no runtime command callback"
        print(f"{name:<14} {priv_size:<11} {f'{nb_in}/{nb_out}':<7} {str(callback):<16} {verdict}")
    print("\nLayout cross-checked against avfilter_filter_pad_count(). A True row means the")
    print("filter has a process_command callback; a False row means a command is rejected")
    print("with AVERROR(ENOSYS) while mpv still reports rc=0 (f_lavfi.c tests `result >= 0`).")
    print("Structural agreement is not behavioural proof: send a command whose value differs")
    print("from the installed one, then re-read the `af` graph to observe the change.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vendor", type=Path,
                        default=Path("apps/desktop/src-tauri/vendor/libmpv/macos"),
                        help="directory holding the packaged FFmpeg dylibs")
    parser.add_argument("--library", type=Path,
                        help="explicit libavfilter path; defaults to <vendor>/libavfilter.dylib")
    parser.add_argument("--filter", action="append", dest="filters",
                        help="filter name; repeatable. Defaults to Echo's P0 chain.")
    args = parser.parse_args()

    if args.library:
        library = args.library
    else:
        suffix = {"darwin": "dylib", "win32": "dll"}.get(sys.platform, "so")
        library = args.vendor / f"libavfilter.{suffix}"
    if not library.is_file():
        print(f"error: {library} not found; pass --library", file=sys.stderr)
        return 1
    return inspect(library, args.filters or DEFAULT_FILTERS)


if __name__ == "__main__":
    raise SystemExit(main())
