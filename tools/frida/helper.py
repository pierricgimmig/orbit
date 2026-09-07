# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Frida Core control plane. Native callbacks never send events through Python."""
import json
import sys


def reply(value):
    print(json.dumps(value), flush=True)


def main():
    import frida
    if frida.__version__ != "17.17.0":
        raise RuntimeError("Orbit requires frida==17.17.0; run tools/frida/build.sh")
    config = json.loads(sys.stdin.readline())
    session = frida.get_local_device().attach(config["pid"])
    session.on("detached", lambda reason, crash: reply({"detached": str(reason)}))
    script = session.create_script(config["script"])
    script.on("message", lambda message, data: print(json.dumps(message), file=sys.stderr, flush=True))
    script.load()
    try:
        if config.get("command") == "symbols":
            reply({"symbols": script.exports_sync.symbols()})
        else:
            reply(script.exports_sync.start(config))
            # Stop on command or EOF (including service death).
            sys.stdin.readline()
    finally:
        try:
            script.exports_sync.stop()
            reply({"stopped": True})
        finally:
            session.detach()


try:
    main()
except Exception as error:
    reply({"error": str(error)})
    sys.exit(1)
