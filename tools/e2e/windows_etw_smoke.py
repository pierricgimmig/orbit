#!/usr/bin/env python3
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Windows kernel-logger smoke test: scheduler, thread states and samples.

Starts perfr, runs PerfrTestRust, records it for a few seconds and
reads the capture back as a bundle: the target's threads must have
scheduling slices, thread-state bars with more than one state, and samples
whose frames resolve into the target's own functions. Needs an elevated
prompt (the NT Kernel Logger); without one the service says so on its
status line and this script skips, unless --require is given (CI does).
Requires pyarrow.
"""
import argparse
import io
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

KIND_SCHEDULING_SLICE = 3
KIND_THREAD_STATE = 4
KIND_SAMPLE = 7
KIND_FUNCTION_CALL = 2


def read_line_within(child, seconds):
    box = []
    reader = threading.Thread(target=lambda: box.append(child.stdout.readline()), daemon=True)
    reader.start()
    reader.join(seconds)
    return box[0].strip() if box else None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--service', required=True)
    parser.add_argument('--target', required=True, help='PerfrTestRust.exe (any instrumented, busy target works)')
    parser.add_argument('--seconds', type=float, default=3.0)
    parser.add_argument('--require', action='store_true', help='fail instead of skipping when the kernel logger is unavailable')
    args = parser.parse_args()
    from pyarrow import parquet
    service_path = str(Path(args.service).resolve())
    target_path = str(Path(args.target).resolve())
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
    base = f'http://127.0.0.1:{port}'

    def request(path, body=None, timeout=60):
        req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(),
                                     headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=timeout) as response:
            data = response.read()
            return json.loads(data) if data.startswith((b'{', b'[')) else data

    api_lib = str(Path(service_path).with_name('perfr_api.dll'))
    env = dict(os.environ)
    if Path(api_lib).is_file():
        env['PERFR_API_LIB'] = api_lib
    with tempfile.TemporaryFile(mode='w+') as log:
        service = subprocess.Popen([service_path, '--host', '127.0.0.1', '--serve', str(port)], stdout=log, stderr=log)
        target = subprocess.Popen([target_path, '--seconds', str(int(args.seconds) + 15)], stdout=subprocess.PIPE, stderr=log, text=True, env=env)
        try:
            announcement = read_line_within(target, 10) or ''
            pid = target.pid
            deadline = time.monotonic() + 30
            while True:
                try:
                    request('/api/status', timeout=1)
                    break
                except OSError:
                    assert service.poll() is None and time.monotonic() < deadline, 'service did not start'
                    time.sleep(.1)
            request('/api/capture/start', {'pid': pid})
            time.sleep(1.0)
            status = request('/api/status')
            line = status.get('instrumentation', '')
            if 'elevated' in line:
                request('/api/capture/stop', {})
                message = f'kernel logger unavailable: {line}'
                if args.require:
                    raise AssertionError(message)
                print('SKIP: ' + message)
                return
            time.sleep(args.seconds)
            request('/api/capture/stop', {})
            status = request('/api/status')
            assert not status['capturing']
            bundle = request('/api/capture/export?format=bundle')
            with zipfile.ZipFile(io.BytesIO(bundle)) as capture:
                manifest = json.loads(capture.read('manifest.json'))
                rows = parquet.read_table(io.BytesIO(capture.read(manifest['files']['events']))).to_pylist()
            ours = [r for r in rows if r['pid'] == pid]
            slices = [r for r in rows if r['kind'] == KIND_SCHEDULING_SLICE]
            our_slices = [r for r in ours if r['kind'] == KIND_SCHEDULING_SLICE]
            states = {r['extra'] for r in ours if r['kind'] == KIND_THREAD_STATE}
            samples = [r for r in ours if r['kind'] == KIND_SAMPLE]
            frames = [r for r in ours if r['kind'] == KIND_FUNCTION_CALL]
            cores = {r['extra'] for r in slices}
            resolved = [r for r in frames if r['name'] and not r['name'].startswith('0x') and not r['name'].startswith('[')]
            print(f'{len(slices)} scheduling slices on {len(cores)} cores ({len(our_slices)} for pid {pid}); '
                  f'thread states {sorted(states)}; {len(samples)} samples with {len(frames)} frames ({len(resolved)} named); '
                  f'target said {announcement!r}')
            assert len(slices) > 100, 'the scheduler track is empty'
            assert our_slices, 'no scheduling slice for the target'
            assert len(cores) >= 2, 'slices on one core only'
            assert 0 in states and len(states) >= 2, f'thread states should include RUNNING and a wait, got {sorted(states)}'
            expected = args.seconds * 1000 * 0.3
            assert len(samples) >= expected, f'{len(samples)} samples in {args.seconds}s; a busy target at 1 kHz should give at least {expected:.0f}'
            assert frames, 'samples carry no frames'
            assert len(resolved) * 2 > len(frames), f'most frames should resolve to a name ({len(resolved)} of {len(frames)})'
            print(f'PASS: scheduler, thread states and {len(samples)} samples from the kernel logger for pid {pid}')
        except BaseException:
            log.seek(0)
            print(log.read(), file=sys.stderr)
            raise
        finally:
            for child in (target, service):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()


if __name__ == '__main__':
    main()
