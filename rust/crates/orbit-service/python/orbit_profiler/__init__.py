# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Orbit, the profiler, as a pip-installable package.

    pip install orbit-profiler
    orbit-service --serve 44766        # then open http://127.0.0.1:44766/

The wheel carries the prebuilt ``orbit-service`` binary -- the capture service
that also serves the web viewer -- plus ``liborbit_api`` and ``orbit.h`` for
instrumenting C and C++, and depends on ``orbit-api`` so ``import orbit_api``
works for Python. The ``orbit-service`` console command runs the bundled
binary; ``orbit_profiler.binary_path()`` returns its location.
"""

import os
import sys

__all__ = ["binary_path", "header_path", "library_path", "main"]
__version__ = "0.1.0"

if sys.platform == "win32":
    _BINARY, _LIBRARY = "orbit-service.exe", "orbit_api.dll"
elif sys.platform == "darwin":
    _BINARY, _LIBRARY = "orbit-service", "liborbit_api.dylib"
else:
    _BINARY, _LIBRARY = "orbit-service", "liborbit_api.so"


def _here(name):
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), name)


def binary_path():
    """The bundled ``orbit-service`` executable."""
    return _here(_BINARY)


def library_path():
    """The bundled ``liborbit_api`` shared library, or ``None`` if absent."""
    p = _here(_LIBRARY)
    return p if os.path.exists(p) else None


def header_path():
    """The bundled ``orbit.h``, or ``None`` if absent."""
    p = _here("orbit.h")
    return p if os.path.exists(p) else None


def main():
    """Entry point for the ``orbit-service`` console script: run the bundled
    binary with this process's arguments, replacing this process where the OS
    allows it so signals and exit codes pass straight through."""
    binary = binary_path()
    if not os.path.exists(binary):
        sys.exit("orbit-profiler: the orbit-service binary is missing from this wheel")
    # Wheels do not record the executable bit; set it once, ignoring a
    # read-only install (a system that pre-set the bit, or a reinstall).
    try:
        os.chmod(binary, 0o755)
    except OSError:
        pass
    argv = [binary] + sys.argv[1:]
    if sys.platform == "win32":
        import subprocess

        sys.exit(subprocess.call(argv))
    os.execv(binary, argv)
