#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Serves the built site (`tools/site/build_site.py`) on the LAN.

Plain `python3 -m http.server` would do, except that the viewer's worker pool
needs SharedArrayBuffer, which browsers only enable on pages served with the
cross-origin isolation headers. This adds them, the right MIME type for
`.wasm`, byte ranges (so a phone can play the landing-page videos), and no
caching, so a rebuilt site shows up on reload.

GitHub Pages cannot send those headers. The built site's
`coi-serviceworker.js` adds them and reloads once. This server already
isolates the document, so the worker sees `crossOriginIsolated` and does
not register. Either way the pool starts when the browser allows it, and
the viewer stays single-threaded when it does not.

    python3 tools/site/serve.py --dir site --port 8081
"""

import argparse
import http.server
import os
from http import HTTPStatus


class _Limited:
    """A file object that stops after `remaining` bytes. Range responses need
    this because SimpleHTTPRequestHandler copies until EOF."""

    def __init__(self, handle, remaining):
        self._handle = handle
        self._remaining = remaining

    def read(self, n=-1):
        if self._remaining <= 0:
            return b""
        if n is None or n < 0 or n > self._remaining:
            n = self._remaining
        data = self._handle.read(n)
        self._remaining -= len(data)
        return data

    def close(self):
        self._handle.close()


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".mp4": "video/mp4",
        ".webm": "video/webm",
        ".stream": "application/octet-stream",
        ".md": "text/markdown; charset=utf-8",
    }

    def end_headers(self):
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def log_message(self, fmt, *args):  # quieter than the default
        if os.environ.get("ORBIT_SITE_LOG"):
            super().log_message(fmt, *args)

    def send_head(self):
        # iOS Safari will not play an MP4 that cannot answer a Range request.
        ranged = self._send_range()
        if ranged is not False:
            return ranged
        return super().send_head()

    def _send_range(self):
        header = self.headers.get("Range")
        if not header:
            return False
        path = self.translate_path(self.path)
        if os.path.isdir(path) or path.endswith("/") or not os.path.isfile(path):
            return False
        try:
            handle = open(path, "rb")
        except OSError:
            self.send_error(HTTPStatus.NOT_FOUND, "File not found")
            return None
        size = os.fstat(handle.fileno()).st_size
        span = _parse_range(header, size)
        if span is None:
            handle.close()
            self.send_response(HTTPStatus.REQUESTED_RANGE_NOT_SATISFIABLE)
            self.send_header("Content-Range", f"bytes */{size}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return None
        start, end = span
        length = end - start + 1
        self.send_response(HTTPStatus.PARTIAL_CONTENT)
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Length", str(length))
        self.send_header("Last-Modified", self.date_time_string(os.path.getmtime(path)))
        self.end_headers()
        handle.seek(start)
        return _Limited(handle, length)


def _parse_range(header, size):
    """The first `bytes=` range, or None if it cannot be satisfied."""
    if size <= 0 or not header.startswith("bytes="):
        return None
    spec = header[6:].split(",", 1)[0].strip()
    if "-" not in spec:
        return None
    start_s, end_s = spec.split("-", 1)
    try:
        if start_s == "":
            length = int(end_s)
            if length <= 0:
                return None
            start = max(0, size - length)
            end = size - 1
        else:
            start = int(start_s)
            end = int(end_s) if end_s else size - 1
    except ValueError:
        return None
    if start < 0 or start >= size or end < start:
        return None
    return start, min(end, size - 1)


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
