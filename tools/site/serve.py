#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Serves the built site (`tools/site/build_site.py`) on the LAN.

Plain `python3 -m http.server` would do, except that the viewer's worker pool
needs SharedArrayBuffer, which browsers only enable on pages served with the
cross-origin isolation headers. This adds them, the right MIME type for
`.wasm`, and no caching, so a rebuilt site shows up on reload.

    python3 tools/site/serve.py --dir site --port 8081
"""

import argparse
import http.server
import os


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".stream": "application/octet-stream",
        ".md": "text/markdown; charset=utf-8",
    }

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def log_message(self, fmt, *args):  # quieter than the default
        if os.environ.get("ORBIT_SITE_LOG"):
            super().log_message(fmt, *args)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", default="site")
    parser.add_argument("--port", type=int, default=8081)
    parser.add_argument("--bind", default="0.0.0.0")
    args = parser.parse_args()
    os.chdir(args.dir)
    server = http.server.ThreadingHTTPServer((args.bind, args.port), Handler)
    print(f"serving {os.getcwd()} on http://{args.bind}:{args.port}/", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
