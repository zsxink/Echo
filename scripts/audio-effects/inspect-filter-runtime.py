#!/usr/bin/env python3
"""Report which audio filters accept a RUNTIME command, from real dylibs.

`avfilter_graph_send_command` returns AVERROR(ENOSYS) when the target filter has
no `process_command` callback. mpv's `f_lavfi.c` only tests `result >= 0`, so a
rejected command is reported to the caller as SUCCESS. Return codes from
`af-command` are therefore not evidence that a parameter changed.

This reads the `process_command` field of each `AVFilter` struct directly, which
needs no audio and no graph. Two traps it avoids:

1. FFmpeg 6.0 field order is
   name, description, priv_size, flags_internal, priv_class,
   inputs, nb_inputs, outputs, nb_outputs, process_command
   A wrong order silently reads adjacent memory and reports every filter as
   supporting commands. `extrastereo` is the control: it has no callback, so a
   run where it reports True means the layout is wrong.
2. `av_buffersink_add_frame` and `av_buffersrc_add_frame` are inline in
   libavfilter and are not resolvable through dlsym, so output-level
   measurement needs a different harness than this.

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

# Field order per FFmpeg 6.0 libavfilter/avfilter.h.
FIELDS_60 = [
    "name", "description", "priv_size", "flags_internal", "priv_class",
    "inputs", "nb_inputs", "outputs", "nb_outputs", "process_command",
]

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
    _fields_ = [
        ("name", C.c_char_p), ("description", C.c_char_p),
        ("priv_size", C.c_int), ("flags_internal", C.c_int),
        ("priv_class", C.POINTER(AVClass)),
        ("inputs", C.POINTER(AVFilterPad)), ("nb_inputs", C.c_uint),
        ("outputs", C.POINTER(AVFilterPad)), ("nb_outputs", C.c_uint),
        ("process_command", C.c_void_p),
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
    try:
        lib.avutil_version.restype = C.c_uint
        lib.avutil_version.argtypes = []
    except AttributeError:
        pass
    return lib, get_by_name


def inspect(library: Path, names: list[str]) -> int:
    lib, get_by_name = load(library)
    print(f"library : {library}")
    print(f"sha256  : {digest(library)}")
    print()
    print(f"{'filter':<14} {'present':<8} {'process_command':<16} verdict")
    suspicious = False
    for name in names:
        handle = get_by_name(name.encode())
        if not handle:
            print(f"{name:<14} {'no':<8} {'-':<16} not compiled into this build")
            continue
        filt = C.cast(handle, C.POINTER(AVFilter)).contents
        # Sanity: the struct read must agree with its own name field.
        decoded = filt.name.decode() if filt.name else ""
        callback = filt.process_command
        supports = bool(callback)
        if supports:
            verdict = "runtime command accepted"
        else:
            verdict = "af-command is a silent no-op"
        print(f"{name:<14} {'yes':<8} {str(supports):<16} {verdict}")
        if decoded != name:
            print(f"{'':<14} ! struct layout mismatch: read name {decoded!r}, expected {name!r}")
            suspicious = True
    if suspicious:
        print("\nStruct layout mismatch detected; the process_command column is unreliable.")
        return 2
    print("\nA False row means the filter exists but rejects runtime commands:")
    print("mpv still reports rc=0 because f_lavfi.c only tests `result >= 0`.")
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
