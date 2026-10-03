"""Small thread-confined ctypes binding to explicit libmpv client.h APIs.

The CDLL and handle outlive all pointers. Events are copied before the next
wait_event. Owned mpv node contents are always freed by mpv_free_node_contents. No callback
or pointer is retained by Python after destroy, and no library search fallback
is used. Run in a timeout-limited subprocess because native calls may block.
"""
import ctypes as C
import time
import os
import sys


class Event(C.Structure):
    _fields_ = [('event_id', C.c_int), ('error', C.c_int),
                ('reply_userdata', C.c_uint64), ('data', C.c_void_p)]


class LogMessage(C.Structure):
    _fields_ = [('prefix', C.c_char_p), ('level', C.c_char_p),
                ('text', C.c_char_p), ('log_level', C.c_int)]


class Node(C.Structure):
    pass


class NodeList(C.Structure):
    _fields_ = [('num', C.c_int), ('values', C.POINTER(Node)), ('keys', C.POINTER(C.c_char_p))]


class NodeValue(C.Union):
    _fields_ = [('string', C.c_char_p), ('flag', C.c_int), ('int64', C.c_int64),
                ('double', C.c_double), ('list', C.POINTER(NodeList)), ('byte_array', C.c_void_p)]


Node._fields_ = [('u', NodeValue), ('format', C.c_int)]


def node_value(node):
    if node.format == 0:
        return None
    if node.format == 1:
        return decode(node.u.string)
    if node.format == 3:
        return bool(node.u.flag)
    if node.format == 4:
        return node.u.int64
    if node.format == 5:
        return node.u.double
    if node.format in (7, 8):
        listing = node.u.list.contents
        values = [node_value(listing.values[i]) for i in range(listing.num)]
        if node.format == 8:
            return {decode(listing.keys[i]): value for i, value in enumerate(values)}
        return values
    return {'unsupported_mpv_format': node.format}


def decode(value):
    return value.decode('utf-8', errors='replace') if value else ''


class Mpv:
    def __init__(self, path, ao='null', ao_pcm_file=None):
        self.loader = None
        self.native_library_handle = None
        if sys.platform != 'win32':
            # ctypes adds RTLD_NOW even when its mode argument includes LAZY.
            # Use dlopen directly to match Rust libloading's RTLD_LAZY|LOCAL.
            self.loader = C.CDLL(None)
            self.loader.dlopen.argtypes = [C.c_char_p, C.c_int]
            self.loader.dlopen.restype = C.c_void_p
            self.loader.dlerror.argtypes = []
            self.loader.dlerror.restype = C.c_char_p
            self.loader.dlclose.argtypes = [C.c_void_p]
            self.loader.dlclose.restype = C.c_int
            self.loader.dlerror()
            self.native_library_handle = self.loader.dlopen(os.fsencode(path), os.RTLD_LAZY | os.RTLD_LOCAL)
            if not self.native_library_handle:
                raise OSError(decode(self.loader.dlerror()))
            self.lib = C.CDLL(str(path), handle=self.native_library_handle)
        else:
            self.lib = C.CDLL(str(path))
        signatures = {
            'mpv_create': (C.c_void_p, []),
            'mpv_client_api_version': (C.c_ulong, []),
            'mpv_initialize': (C.c_int, [C.c_void_p]),
            'mpv_set_option_string': (C.c_int, [C.c_void_p, C.c_char_p, C.c_char_p]),
            'mpv_set_property_string': (C.c_int, [C.c_void_p, C.c_char_p, C.c_char_p]),
            'mpv_get_property_string': (C.c_void_p, [C.c_void_p, C.c_char_p]),
            'mpv_get_property': (C.c_int, [C.c_void_p, C.c_char_p, C.c_int, C.c_void_p]),
            'mpv_free_node_contents': (None, [C.POINTER(Node)]),
            'mpv_command': (C.c_int, [C.c_void_p, C.POINTER(C.c_char_p)]),
            'mpv_wait_event': (C.POINTER(Event), [C.c_void_p, C.c_double]),
            'mpv_request_log_messages': (C.c_int, [C.c_void_p, C.c_char_p]),
            'mpv_error_string': (C.c_char_p, [C.c_int]),
            'mpv_free': (None, [C.c_void_p]),
            'mpv_terminate_destroy': (None, [C.c_void_p]),
        }
        for name, (result, arguments) in signatures.items():
            function = getattr(self.lib, name)
            function.restype, function.argtypes = result, arguments
        self.handle = self.lib.mpv_create()
        if not self.handle:
            raise RuntimeError('mpv_create returned NULL')
        self.operations, self.events = [], []
        self.started = time.monotonic()
        try:
            options = [('config', 'no'), ('load-scripts', 'no'), ('terminal', 'no'),
                       ('vid', 'no'), ('vo', 'null'), ('idle', 'yes'),
                       ('keep-open', 'yes'), ('ao', ao), ('audio-channels', 'auto')]
            if ao == 'pcm':
                if not ao_pcm_file:
                    raise ValueError('ao=pcm requires an explicit PCM/WAVE output path')
                options.extend([('ao-pcm-file', os.fspath(ao_pcm_file)), ('ao-pcm-waveheader', 'yes')])
            for name, value in options:
                result = self.lib.mpv_set_option_string(self.handle, name.encode(), value.encode())
                self.record('option', [name, value], result)
                if result < 0 and not (name == 'load-scripts' and result == -5):
                    raise RuntimeError(f'Cannot set required option {name}: {self.error(result)}')
            result = self.lib.mpv_request_log_messages(self.handle, b'debug')
            self.record('request-log', ['debug'], result)
            result = self.lib.mpv_initialize(self.handle)
            self.record('initialize', [], result)
            if result < 0:
                raise RuntimeError(self.error(result))
            # The hardware-output diagnostic uses the synthetic -60 dBFS
            # fixture. Keep the host speaker volume low throughout the run.
            if ao == 'coreaudio':
                result = self.lib.mpv_set_property_string(self.handle, b'volume', b'1')
                self.record('property', ['volume', '1'], result)
                if result < 0:
                    raise RuntimeError(f'Cannot set low diagnostic volume: {self.error(result)}')
        except BaseException:
            self.close()
            raise

    def error(self, code):
        return decode(self.lib.mpv_error_string(code))

    def record(self, kind, args, result):
        self.operations.append({'elapsed_seconds': time.monotonic() - self.started,
                                'kind': kind, 'args': args, 'result': result,
                                'error': self.error(result) if result < 0 else None})
        return result

    def command(self, *args):
        array = (C.c_char_p * (len(args) + 1))(*[arg.encode() for arg in args], None)
        return self.record('command', list(args), self.lib.mpv_command(self.handle, array))

    def set(self, name, value):
        return self.record('property', [name, value], self.lib.mpv_set_property_string(
            self.handle, name.encode(), value.encode()))

    def get(self, name):
        node = Node()
        code = self.lib.mpv_get_property(self.handle, name.encode(), 6, C.byref(node))
        if code < 0:
            return None
        try:
            return node_value(node)
        finally:
            self.lib.mpv_free_node_contents(C.byref(node))

    def pump(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            event = self.lib.mpv_wait_event(self.handle, min(0.02, max(0, deadline - time.monotonic()))).contents
            if event.event_id == 0:
                continue
            copied = {'elapsed_seconds': time.monotonic() - self.started,
                      'id': event.event_id, 'error': event.error}
            if event.event_id == 2 and event.data:
                log = C.cast(event.data, C.POINTER(LogMessage)).contents
                copied.update(prefix=decode(log.prefix), level=decode(log.level), text=decode(log.text))
            self.events.append(copied)

    def close(self):
        if self.handle:
            self.lib.mpv_terminate_destroy(self.handle)
            self.handle = None
        if self.native_library_handle:
            self.loader.dlclose(self.native_library_handle)
            self.native_library_handle = None
