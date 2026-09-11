# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Orbit manual instrumentation for Python.

    import orbit_api as orbit

    orbit.init()                      # finds liborbit_api, or stays quiet
    with orbit.scope("update"):
        ...
    orbit.value("fps", 59.9)

    @orbit.scope                      # or @orbit.scope("a name")
    def render(frame): ...

Pure Python, no dependencies, no compiled wheel. The producer -- the
shared-memory ring a running orbit-service drains -- is the same
``liborbit_api`` that C and C++ load through ``orbit.h``, found by the same
rules and loaded with ctypes. Doing the ring in Python instead would be slower
and, on ARM, wrong: the claim is an atomic add and the commit a release store,
and Python can emit neither.

Where the library is looked for, in order:

1. ``$ORBIT_API_LIB``: the full path of the library file. When set it is the
   only place tried, so a wrong path fails visibly.
2. beside ``orbit-service``: the directory of the ``orbit-service`` on
   ``PATH``, then ``~/.local/bin`` (the install script's default) and
   ``~/.orbit/bin``. The service is the authority on the ring protocol
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
    _LIB_FILE, _SERVICE_FILE = "orbit_api.dll", "orbit-service.exe"
elif sys.platform == "darwin":
    _LIB_FILE, _SERVICE_FILE = "liborbit_api.dylib", "orbit-service"
else:
    _LIB_FILE, _SERVICE_FILE = "liborbit_api.so", "orbit-service"


def _candidates():
    # The service is the authority on the ring's protocol version: it refuses a
    # segment whose version is not its own, so the library that writes must
    # match the service that reads. When a service is reachable, its own
    # library is therefore preferred over the copy bundled in this wheel, which
    # matters only when the two drift (a pinned wheel against a newer service).
    # With ORBIT_API_LIB set, that path is the only one tried, so a wrong path
    # fails visibly instead of loading some other copy.
    env = os.environ.get("ORBIT_API_LIB")
    if env:
        yield env
        return
    service = shutil.which(_SERVICE_FILE)
    if service:
        yield os.path.join(os.path.dirname(os.path.realpath(service)), _LIB_FILE)
    home = os.path.expanduser("~")
    for sub in ((".orbit", "bin"),) if sys.platform == "win32" else ((".local", "bin"), (".orbit", "bin")):
        yield os.path.join(home, *sub, _LIB_FILE)
    # The copy shipped inside this wheel, always version-matched to the package.
    yield os.path.join(os.path.dirname(os.path.abspath(__file__)), _LIB_FILE)
    found = ctypes.util.find_library("orbit_api")
    if found:
        yield found
    yield _LIB_FILE


def _open(path):
    # PyDLL keeps the GIL across the call. The functions never block, never
    # call back into Python and take about fifteen nanoseconds; dropping and
    # retaking the GIL around each would cost more than the call.
    lib = ctypes.PyDLL(path)
    lib.orbit_api_table_v1.argtypes = (ctypes.c_uint32,)
    lib.orbit_api_table_v1.restype = ctypes.c_void_p
    if not lib.orbit_api_table_v1(ABI_VERSION):
        raise OSError(E_ABI, "liborbit_api speaks another ABI")
    lib.orbit_init.argtypes = ()
    lib.orbit_init.restype = ctypes.c_int
    lib.orbit_shutdown.argtypes = ()
    lib.orbit_shutdown.restype = None
    for fn in (lib.orbit_start, lib.orbit_start_async, lib.orbit_instant):
        fn.argtypes = (ctypes.c_char_p, ctypes.c_size_t)
        fn.restype = ctypes.c_uint64
    lib.orbit_stop.argtypes = (ctypes.c_uint64,)
    lib.orbit_stop.restype = None
    lib.orbit_link.argtypes = (ctypes.c_uint64, ctypes.c_uint64)
    lib.orbit_link.restype = None
    lib.orbit_value.argtypes = (ctypes.c_char_p, ctypes.c_size_t, ctypes.c_double)
    lib.orbit_value.restype = None
    lib.orbit_now_ns.argtypes = ()
    lib.orbit_now_ns.restype = ctypes.c_uint64
    for fn in (lib.orbit_span, lib.orbit_span_async):
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
    """Finds and loads ``liborbit_api``, then creates this process's segment
    so a running ``orbit-service`` can find it. Idempotent.

    Returns 0 on success, ``E_NOLIB`` or ``E_ABI`` when no usable library was
    found, or a negative errno if the segment could not be created. Every
    other call stays a valid no-op in each case."""
    lib = _load()
    return lib.orbit_init() if lib else _lib_error


def shutdown():
    """Removes the segment's name. Safe to skip; the service sweeps segments
    of exited processes."""
    if _lib:
        _lib.orbit_shutdown()


def available():
    """Whether a library is loaded, i.e. ``init()`` found one."""
    return _lib is not None


def library_path():
    """The library ``init()`` loaded, or ``None``. For diagnostics."""
    return _lib_path


def start(name):
    """Begins a scope on the calling thread; returns its handle."""
    b = _name(name)
    return _lib.orbit_start(b, len(b)) if _lib else 0


def start_async(name):
    """Begins a scope that may be stopped from any thread, on its own track."""
    b = _name(name)
    return _lib.orbit_start_async(b, len(b)) if _lib else 0


def stop(handle):
    """Ends a scope, from any thread. ``0`` is a no-op."""
    if _lib and handle:
        _lib.orbit_stop(handle)


def instant(name):
    """A point in time with a name and no duration; returns a handle so it
    can be either end of a link."""
    b = _name(name)
    return _lib.orbit_instant(b, len(b)) if _lib else 0


def link(src, dst):
    """An arrow from one event to another, across threads if need be."""
    if _lib and src and dst:
        _lib.orbit_link(src, dst)


def value(name, v):
    """A value to graph over time on a track named ``name``."""
    if _lib:
        b = _name(name)
        _lib.orbit_value(b, len(b), float(v))


def now_ns():
    """``CLOCK_MONOTONIC`` nanoseconds, the clock of every timestamp; ``0``
    while no library is loaded."""
    return _lib.orbit_now_ns() if _lib else 0


def span(name, start_ns, end_ns):
    """A complete scope at timestamps you supply (``now_ns()``'s clock)."""
    if _lib:
        b = _name(name)
        _lib.orbit_span(b, len(b), int(start_ns), int(end_ns))


def span_async(name, start_ns, end_ns):
    """A complete async span at supplied timestamps, on its own track."""
    if _lib:
        b = _name(name)
        _lib.orbit_span_async(b, len(b), int(start_ns), int(end_ns))


class scope:
    """A scope as a context manager or a decorator.

        with orbit.scope("update") as s:   # s.handle is the handle
            ...

        @orbit.scope("render")             # or bare @orbit.scope: the
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
