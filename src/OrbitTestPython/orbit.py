# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Orbit manual instrumentation for Python, over the eight-function C ABI.

    import orbit
    orbit.init()
    with orbit.scope("update"):
        ...
    orbit.value("fps", 59.9)

Names cross as bytes with an explicit length, so nothing is scanned or copied
on the way in. Handles are plain ints. Every call is a no-op when the library
is missing or init was not called, the same as in C.
"""

import ctypes
import os
import sys

_lib = None


def _find_library():
    candidates = [os.environ.get("ORBIT_API_LIB")]
    here = os.path.dirname(os.path.abspath(__file__))
    for rel in ("../../rust/target/release", "../../rust/target/debug"):
        candidates.append(os.path.join(here, rel, "liborbit_api.so"))
    for path in candidates:
        if path and os.path.exists(path):
            return path
    return None


def _load():
    global _lib
    if _lib is not None:
        return _lib
    path = _find_library()
    if path is None:
        return None
    lib = ctypes.CDLL(path)
    lib.orbit_init.restype = ctypes.c_int
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
    _lib = lib
    return lib


def _name(name):
    return name if isinstance(name, bytes) else name.encode("utf-8")


def init():
    """Creates this process's segment. Returns 0, a negative errno, or None
    if the library could not be found."""
    lib = _load()
    return lib.orbit_init() if lib else None


def shutdown():
    if _lib:
        _lib.orbit_shutdown()


def start(name):
    b = _name(name)
    return _lib.orbit_start(b, len(b)) if _lib else 0


def start_async(name):
    b = _name(name)
    return _lib.orbit_start_async(b, len(b)) if _lib else 0


def stop(handle):
    if _lib and handle:
        _lib.orbit_stop(handle)


def instant(name):
    b = _name(name)
    return _lib.orbit_instant(b, len(b)) if _lib else 0


def link(src, dst):
    if _lib and src and dst:
        _lib.orbit_link(src, dst)


def value(name, v):
    if _lib:
        b = _name(name)
        _lib.orbit_value(b, len(b), float(v))


class scope:
    """`with orbit.scope("name") as s:` -- `s.handle` is the handle."""

    __slots__ = ("name", "handle", "_async")

    def __init__(self, name, async_=False):
        self.name = name
        self.handle = 0
        self._async = async_

    def __enter__(self):
        self.handle = start_async(self.name) if self._async else start(self.name)
        return self

    def __exit__(self, *exc):
        stop(self.handle)
        return False


def scope_async(name):
    return scope(name, async_=True)


if __name__ == "__main__":
    print("library:", _find_library(), file=sys.stderr)
