# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""perfr, the profiler, as a pip-installable package.

    pip install perfr
    perfr                      # then open http://127.0.0.1:44766/

The wheel carries the prebuilt ``perfr`` binary -- the capture service
that also serves the web viewer -- plus ``libperfr_api`` and ``perfr.h`` for
instrumenting C and C++, the Frida agent and helper the service loads from
the same directory, and a dependency on ``perfr-api`` so ``import perfr_api``
works for Python. The ``perfr`` console command runs the bundled
binary; ``perfr.binary_path()`` returns its location.
"""

import os
import sys

__all__ = ["binary_path", "header_path", "library_path", "main"]
__version__ = "0.1.0"

if sys.platform == "win32":
    _BINARY, _LIBRARY = "perfr.exe", "perfr_api.dll"
elif sys.platform == "darwin":
    _BINARY, _LIBRARY = "perfr", "libperfr_api.dylib"
else:
    _BINARY, _LIBRARY = "perfr", "libperfr_api.so"


def _here(name):
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), name)


def binary_path():
    """The bundled ``perfr`` executable."""
    return _here(_BINARY)


def library_path():
    """The bundled ``libperfr_api`` shared library, or ``None`` if absent."""
    p = _here(_LIBRARY)
    return p if os.path.exists(p) else None


def header_path():
    """The bundled ``perfr.h``, or ``None`` if absent."""
    p = _here("perfr.h")
    return p if os.path.exists(p) else None


def _ensure_executable(path):
    """Wheels do not keep the executable bit. A read-only install is ignored."""
    if not os.path.exists(path):
        return
    try:
        os.chmod(path, 0o755)
    except OSError:
        pass


def main():
    """Entry point for the ``perfr`` console script: run the bundled
    binary with this process's arguments, replacing this process where the OS
    allows it so signals and exit codes pass straight through."""
    binary = binary_path()
    if not os.path.exists(binary):
        sys.exit("perfr: the perfr binary is missing from this wheel")
    # The service spawns perfr-frida-helper from the same directory. Both
    # need the executable bit, which a wheel does not record.
    _ensure_executable(binary)
    _ensure_executable(_here("perfr-frida-helper"))
    argv = [binary] + sys.argv[1:]
    if sys.platform == "win32":
        import subprocess

        sys.exit(subprocess.call(argv))
    os.execv(binary, argv)
