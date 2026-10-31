# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""perfr manual instrumentation for Python.

    import perfr_api as perfr

    perfr.init()                      # finds libperfr_api, or stays quiet
    with perfr.scope("update"):
        ...
    perfr.value("fps", 59.9)

    @perfr.scope                      # or @perfr.scope("a name")
    def render(frame): ...

Pure Python, no dependencies, no compiled wheel. The producer -- the
shared-memory ring a running perfr drains -- is the same
``libperfr_api`` that C and C++ load through ``perfr.h``, found by the same
rules and loaded with ctypes. Doing the ring in Python instead would be slower
and, on ARM, wrong: the claim is an atomic add and the commit a release store,
and Python can emit neither.

Where the library is looked for, in order:

1. ``$PERFR_API_LIB``: the full path of the library file. When set it is the
   only place tried, so a wrong path fails visibly.
2. beside ``perfr``: the directory of the ``perfr`` on
   ``PATH``, then ``~/.local/bin`` (the install script's default) and
   ``~/.perfr/bin``. The service is the authority on the ring protocol
   version, so its own library wins over any other copy.
3. beside this package: the copy bundled in the wheel, always matched to
   this package's version.
4. the system loader.

Every call is a no-op when the library is missing or ``init()`` was not
called, the same as in C. Names may be ``str`` or ``bytes``; ``bytes`` skips
an encode on the hot path. Handles are plain ints and ``0`` means "no event".
"""

import ctypes
import ctypes.util
import functools
import os
import shutil
import sys

__all__ = [
    "ABI_VERSION", "E_ABI", "E_NOLIB", "available", "init", "instant", "library_path",
    "link", "now_ns", "scope", "scope_async", "shutdown", "span", "span_async", "start",
    "start_async", "stop", "value",
]
__version__ = "0.1.0"

#: The ABI this package speaks; the library hands out its table only for it.
ABI_VERSION = 1
#: ``init()`` results beyond 0 (ok) and a negative errno.
E_NOLIB = -1000
E_ABI = -1001

_lib = None
_lib_path = None
_lib_error = None

if sys.platform == "win32":
    _LIB_FILE, _SERVICE_FILE = "perfr_api.dll", "perfr.exe"
elif sys.platform == "darwin":
    _LIB_FILE, _SERVICE_FILE = "libperfr_api.dylib", "perfr"
else:
    _LIB_FILE, _SERVICE_FILE = "libperfr_api.so", "perfr"


def _candidates():
    # The service is the authority on the ring's protocol version: it refuses a
    # segment whose version is not its own, so the library that writes must
    # match the service that reads. When a service is reachable, its own
    # library is therefore preferred over the copy bundled in this wheel, which
    # matters only when the two drift (a pinned wheel against a newer service).
    # With PERFR_API_LIB set, that path is the only one tried, so a wrong path
    # fails visibly instead of loading some other copy.
    env = os.environ.get("PERFR_API_LIB")
    if env:
        yield env
        return
    service = shutil.which(_SERVICE_FILE)
    if service:
        yield os.path.join(os.path.dirname(os.path.realpath(service)), _LIB_FILE)
    home = os.path.expanduser("~")
    for sub in ((".perfr", "bin"),) if sys.platform == "win32" else ((".local", "bin"), (".perfr", "bin")):
        yield os.path.join(home, *sub, _LIB_FILE)
    # The copy shipped inside this wheel, always version-matched to the package.
    yield os.path.join(os.path.dirname(os.path.abspath(__file__)), _LIB_FILE)
    found = ctypes.util.find_library("perfr_api")
    if found:
        yield found
    yield _LIB_FILE


def _open(path):
    # PyDLL keeps the GIL across the call. The functions never block, never
    # call back into Python and take about fifteen nanoseconds; dropping and
    # retaking the GIL around each would cost more than the call.
    lib = ctypes.PyDLL(path)
    lib.perfr_api_table_v1.argtypes = (ctypes.c_uint32,)
    lib.perfr_api_table_v1.restype = ctypes.c_void_p
    if not lib.perfr_api_table_v1(ABI_VERSION):
        raise OSError(E_ABI, "libperfr_api speaks another ABI")
    lib.perfr_init.argtypes = ()
    lib.perfr_init.restype = ctypes.c_int
    lib.perfr_shutdown.argtypes = ()
    lib.perfr_shutdown.restype = None
    for fn in (lib.perfr_start, lib.perfr_start_async, lib.perfr_instant):
        fn.argtypes = (ctypes.c_char_p, ctypes.c_size_t)
        fn.restype = ctypes.c_uint64
    lib.perfr_stop.argtypes = (ctypes.c_uint64,)
    lib.perfr_stop.restype = None
    lib.perfr_link.argtypes = (ctypes.c_uint64, ctypes.c_uint64)
    lib.perfr_link.restype = None
    lib.perfr_value.argtypes = (ctypes.c_char_p, ctypes.c_size_t, ctypes.c_double)
    lib.perfr_value.restype = None
    lib.perfr_now_ns.argtypes = ()
    lib.perfr_now_ns.restype = ctypes.c_uint64
    for fn in (lib.perfr_span, lib.perfr_span_async):
        fn.argtypes = (ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint64, ctypes.c_uint64)
        fn.restype = None
    return lib


def _load():
    global _lib, _lib_path, _lib_error
    if _lib is not None or _lib_error is not None:
        return _lib
    for path in _candidates():
        if not os.path.isabs(path) or os.path.exists(path):
            try:
                _lib = _open(path)
                _lib_path = path
                return _lib
            except OSError as error:
                if getattr(error, "errno", None) == E_ABI:
                    _lib_error = E_ABI
                    return None
                continue
    _lib_error = E_NOLIB
    return None


def _name(name):
    return name if isinstance(name, bytes) else name.encode("utf-8")


def init():
    """Finds and loads ``libperfr_api``, then creates this process's segment
    so a running ``perfr`` can find it. Idempotent.

    Returns 0 on success, ``E_NOLIB`` or ``E_ABI`` when no usable library was
    found, or a negative errno if the segment could not be created. Every
    other call stays a valid no-op in each case."""
    lib = _load()
    return lib.perfr_init() if lib else _lib_error


def shutdown():
    """Removes the segment's name. Safe to skip; the service sweeps segments
    of exited processes."""
    if _lib:
        _lib.perfr_shutdown()


def available():
    """Whether a library is loaded, i.e. ``init()`` found one."""
    return _lib is not None


def library_path():
    """The library ``init()`` loaded, or ``None``. For diagnostics."""
    return _lib_path


def start(name):
    """Begins a scope on the calling thread; returns its handle."""
    b = _name(name)
    return _lib.perfr_start(b, len(b)) if _lib else 0


def start_async(name):
    """Begins a scope that may be stopped from any thread, on its own track."""
    b = _name(name)
    return _lib.perfr_start_async(b, len(b)) if _lib else 0


def stop(handle):
    """Ends a scope, from any thread. ``0`` is a no-op."""
    if _lib and handle:
        _lib.perfr_stop(handle)


def instant(name):
    """A point in time with a name and no duration; returns a handle so it
    can be either end of a link."""
    b = _name(name)
    return _lib.perfr_instant(b, len(b)) if _lib else 0


def link(src, dst):
    """An arrow from one event to another, across threads if need be."""
    if _lib and src and dst:
        _lib.perfr_link(src, dst)


def value(name, v):
    """A value to graph over time on a track named ``name``."""
    if _lib:
        b = _name(name)
        _lib.perfr_value(b, len(b), float(v))


def now_ns():
    """``CLOCK_MONOTONIC`` nanoseconds, the clock of every timestamp; ``0``
    while no library is loaded."""
    return _lib.perfr_now_ns() if _lib else 0


def span(name, start_ns, end_ns):
    """A complete scope at timestamps you supply (``now_ns()``'s clock)."""
    if _lib:
        b = _name(name)
        _lib.perfr_span(b, len(b), int(start_ns), int(end_ns))


def span_async(name, start_ns, end_ns):
    """A complete async span at supplied timestamps, on its own track."""
    if _lib:
        b = _name(name)
        _lib.perfr_span_async(b, len(b), int(start_ns), int(end_ns))


class scope:
    """A scope as a context manager or a decorator.

        with perfr.scope("update") as s:   # s.handle is the handle
            ...

        @perfr.scope("render")             # or bare @perfr.scope: the
        def render(frame): ...             # function's qualified name
    """

    __slots__ = ("name", "handle", "_async")

    def __new__(cls, name=None, async_=False):
        if callable(name):  # bare @scope: decorate right away
            return cls(None, async_)(name)
        return super().__new__(cls)

    def __init__(self, name=None, async_=False):
        self.name = b"" if name is None else _name(name)
        self.handle = 0
        self._async = async_

    def __enter__(self):
        self.handle = start_async(self.name) if self._async else start(self.name)
        return self

    def __exit__(self, *exc):
        stop(self.handle)
        return False

    def __call__(self, fn):
        name = self.name if self.name is not None else _name(fn.__qualname__)
        starter = start_async if self._async else start

        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            handle = starter(name)
            try:
                return fn(*args, **kwargs)
            finally:
                stop(handle)

        return wrapper


def scope_async(name=None):
    """``scope`` for a scope that may be stopped from any thread."""
    return scope(name, async_=True)
