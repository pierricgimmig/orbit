#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Exercise Share with a local S3 CLI stand-in and a real browser, no AWS writes.

Requires the built rust/crates/orbit-service/target/debug/orbit-service,
google-chrome, openssl and the rebuilt viewer pack. Uses only the standard library.
"""
import functools
import http.server
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import zipfile

from cdp import Chrome

ROOT = Path(__file__).resolve().parents[2]


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait_until(test, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = test()
            if value:
                return value
        except (OSError, ValueError):
            pass
        time.sleep(.1)
    raise AssertionError('Timed out waiting for test condition')


def main():
    service = chrome = https = None
    with tempfile.TemporaryDirectory(prefix='orbit-share-test-') as tmp:
        work = Path(tmp)
        objects = work / 'objects'
        objects.mkdir()
        (objects / 'viewer').symlink_to(ROOT / 'src/OrbitLiveViewer/viewer-dist', target_is_directory=True)
        s3_port, service_port, chrome_port = port(), port(), port()
        viewer = f'https://127.0.0.1:{s3_port}/viewer/index.html'
        object_base = f'https://localhost:{s3_port}'  # Different origin: browser must pass CORS.
        cert, key = work / 'cert.pem', work / 'key.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
                        '-keyout', str(key), '-out', str(cert), '-days', '1', '-subj', '/CN=localhost'],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        class Handler(http.server.SimpleHTTPRequestHandler):
            def end_headers(self):
                self.send_header('Access-Control-Allow-Origin', f'https://127.0.0.1:{s3_port}')
                self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
                self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
                super().end_headers()

            def log_message(self, *_):
                pass

        https = http.server.ThreadingHTTPServer(('127.0.0.1', s3_port), functools.partial(Handler, directory=str(objects)))
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        https.socket = context.wrap_socket(https.socket, server_side=True)
        threading.Thread(target=https.serve_forever, daemon=True).start()
        mock = work / 'aws'
        mock.write_text('''#!/usr/bin/env python3
import os, sys
from pathlib import Path
args = sys.argv[sys.argv.index('s3') + 1:]
command = args[0]
uri = args[2] if command == 'cp' else args[1]
key = uri.split('/', 3)[3]
path = Path(os.environ['MOCK_S3_DIR']) / key
if command == 'cp':
    assert args[1] == '-' and '--content-type' in args
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(sys.stdin.buffer.read())
elif command == 'presign':
    print(os.environ['MOCK_S3_URL'] + '/' + key + '?signature=a%2Fb&token=a+b')
elif command == 'rm':
    path.unlink(missing_ok=True)
else:
    raise SystemExit(2)
''')
        mock.chmod(0o755)
        env = {**os.environ, 'PATH': str(work) + os.pathsep + os.environ['PATH'],
               'ORBIT_SHARE_BUCKET': 'test-bucket', 'ORBIT_SHARE_VIEWER_URL': viewer,
               'MOCK_S3_DIR': str(objects), 'MOCK_S3_URL': object_base,
               'ORBIT_LOG_DIR': str(work / 'logs')}
        env.pop('ORBIT_SHARE_PUBLIC_BASE_URL', None)
        env.pop('ORBIT_LIVE_DEV', None)
        base = f'http://127.0.0.1:{service_port}'

        def post(path, body=None):
            req = urllib.request.Request(base + path, data=json.dumps(body or {}).encode(),
                                         headers={'Content-Type': 'application/json'}, method='POST')
            with urllib.request.urlopen(req, timeout=30) as reply:
                return json.load(reply)

        try:
            with (work / 'service.log').open('w') as log:
                service = subprocess.Popen([str(ROOT / 'rust/crates/orbit-service/target/debug/orbit-service'),
                                            '--host', '127.0.0.1', '--serve', str(service_port)],
                                           env=env, stdout=log, stderr=log)
            wait_until(lambda: urllib.request.urlopen(base + '/api/status', timeout=1).status == 200)
            post('/api/events', {
                'processes': [{'pid': 777, 'name': 'Shared process'}],
                'threads': [{'pid': 777, 'tid': 778, 'name': 'Shared thread'}],
                'spans': [{'pid': 777, 'tid': 778, 'name': name, 'start_ns': start,
                           'duration_ns': duration, 'depth': depth}
                          for name, start, duration, depth in [('Crossing scope', 100, 200, 0),
                                                              ('Inside scope', 160, 10, 1),
                                                              ('Outside scope', 10, 5, 0)]]})
            shared = post('/api/capture/share?t0=180&t1=150')
            assert shared['expires_in'] > 0
            query = urllib.parse.parse_qs(urllib.parse.urlsplit(shared['viewer_url']).query)
            assert query['capture'][0].endswith('?signature=a%2Fb&token=a+b')
            assert query['download'][0] == shared['download_url']
            archive_key = urllib.parse.urlsplit(shared['download_url']).path.lstrip('/')
            archive = objects / archive_key
            local = urllib.request.urlopen(base + '/api/capture/export?format=bundle&t0=150&t1=180').read()
            assert archive.read_bytes() == local, 'Shared archive differs from local slice'
            with zipfile.ZipFile(archive) as bundle:
                manifest = json.loads(bundle.read('manifest.json'))
                assert manifest['rows']['events'] == 2
                assert manifest['bundle']['slice_ns'] == {'start': 150, 'end': 180}
                assert manifest['time_bounds_ns'] == {'start': 100, 'end': 300}
            chrome = Chrome(port=chrome_port)
            chrome.call('Security.setIgnoreCertificateErrors', ignore=True)
            chrome.goto(shared['viewer_url'], settle=1)
            state = wait_until(lambda: chrome.eval('window.__orbit_sel ? JSON.parse(window.__orbit_sel) : null'))
            state = wait_until(lambda: (lambda s: s if s and s.get('events') == 2 else None)(
                chrome.eval('window.__orbit_sel ? JSON.parse(window.__orbit_sel) : null')))
            assert chrome.eval("document.querySelector('a[download]').href") == shared['download_url']
            assert chrome.screenshot(str(work / 'shared.png')) > 20000, 'Viewer painted a blank screenshot'
            chrome.goto(base, settle=1)
            def rect(label):
                rows = chrome.eval("JSON.parse(window.__orbit_ui || '[]')") or []
                return next((row[1:] for row in rows if row[0] == label), None)
            x, y, width, height = wait_until(lambda: rect('Share'))
            chrome.click(x + width / 2, y + height / 2)
            wait_until(lambda: rect('Copy link'))
            archives = list((objects / 'captures').glob('*.orbit.zip'))
            assert len(archives) == 2, 'The Share button did not upload a new capture'
            full = next(a for a in archives if a != archive)
            with zipfile.ZipFile(full) as bundle:
                assert json.loads(bundle.read('manifest.json'))['rows']['events'] == 3
            print('PASS: one-click Share, S3 CLI upload, exact cropped archive, encoded signed URL, cross-origin WASM load, 2 retained scopes and archive download')
        finally:
            if chrome:
                chrome.close()
            if service:
                service.terminate()
                service.wait(timeout=10)
            if https:
                https.shutdown()
                https.server_close()


if __name__ == '__main__':
    main()
