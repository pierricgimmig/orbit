#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Two services on one box, one timeline: the multi-machine smoke (requires pyarrow).

A hub serves the viewer; a second service joins it with --join. The test
checks that the link comes up with a clock fit, that spans posted to the
remote land in the hub's ring under the remote's machine index on the hub's
clock (on one box the true offset is zero, so the conversion error is the
sync error), that the remote's processes show in the hub's list as
`name @host`, and that a Record on the hub for one of them captures it on the
remote and streams its scopes back. No root or browser.
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

KIND_API_SCOPE = 1
MACHINE_SHIFT = 24


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def requester(base):
    def request(path, body=None, timeout=60):
        req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(),
                                     headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=timeout) as response:
            data = response.read()
            return json.loads(data) if data.startswith((b'{', b'[')) else data
    return request


def wait_for(what, predicate, seconds, children=()):
    deadline = time.monotonic() + seconds
    while True:
        try:
            value = predicate()
        except OSError:
            value = None
        if value:
            return value
        for child in children:
            assert child.poll() is None, f'{what}: a service exited with {child.returncode}'
        assert time.monotonic() < deadline, f'{what}: not within {seconds} s'
        time.sleep(0.1)


def read_line_within(child, seconds):
    box = []
    reader = threading.Thread(target=lambda: box.append(child.stdout.readline()), daemon=True)
    reader.start()
    reader.join(seconds)
    return box[0].strip() if box else None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--service', required=True)
    parser.add_argument('--target', required=True, help='OrbitTestRust (any instrumented target works)')
    parser.add_argument('--seconds', type=float, default=2.0)
    parser.add_argument('--band-us', type=float, default=1000.0,
                        help='the alignment error allowed for spans crossing the link on one box')
    args = parser.parse_args()
    from pyarrow import parquet
    service_path = str(Path(args.service).resolve())
    target_path = str(Path(args.target).resolve())
    hub_port, remote_port = free_port(), free_port()
    hub = requester(f'http://127.0.0.1:{hub_port}')
    remote = requester(f'http://127.0.0.1:{remote_port}')

    # On Windows a service finds instrumented processes under
    # %LOCALAPPDATA%\Orbit\scopes. Give the remote and its target a directory
    # of their own, so the hub can only see the target through the link; on
    # Linux and macOS every service on the box shares the segments, and the
    # presence check below holds either way.
    remote_env = dict(os.environ)
    scratch = tempfile.mkdtemp(prefix='orbit-remote-')
    if sys.platform == 'win32':
        remote_env['LOCALAPPDATA'] = scratch
    api_lib = next((p for p in (Path(service_path).with_name(n) for n in ('orbit_api.dll', 'liborbit_api.so', 'liborbit_api.dylib')) if p.is_file()), None)
    if api_lib:
        remote_env['ORBIT_API_LIB'] = str(api_lib)

    with tempfile.TemporaryFile(mode='w+') as hub_log, tempfile.TemporaryFile(mode='w+') as remote_log:
        hub_proc = subprocess.Popen([service_path, '--host', '127.0.0.1', '--serve', str(hub_port)], stdout=hub_log, stderr=hub_log)
        remote_proc = subprocess.Popen([service_path, '--host', '127.0.0.1', '--serve', str(remote_port), '--join', f'127.0.0.1:{hub_port}'],
                                       stdout=remote_log, stderr=remote_log, env=remote_env)
        target = None
        try:
            services = (hub_proc, remote_proc)
            wait_for('hub up', lambda: hub('/api/status', timeout=1), 30, services)
            wait_for('remote up', lambda: remote('/api/status', timeout=1), 30, services)
            # The link, and a fit from at least a few probes.
            peers = wait_for('link with a clock fit', lambda: (lambda p: p if p['peers'] and p['peers'][0]['probes'] >= 8 else None)(hub('/api/peers', timeout=1)), 30, services)
            peer = peers['peers'][0]
            machine = peer['machine']
            host = peer['host']
            assert machine == 1, peers
            assert peer['link'] == 'joined', peers
            print(f"link: machine {machine} = {host!r}, {peer['cores']} cores at lanes {peer['core_base']}..; "
                  f"offset {peer['offset_ns']} ns, rate {peer['rate_ppm']:.2f} ppm, best rtt {peer['rtt_ns']} ns over {peer['probes']} probes")
            assert abs(peer['offset_ns']) < args.band_us * 1000, f"two services on one box read {peer['offset_ns']} ns apart"
            assert hub('/api/status')['peers'] == 1
            assert remote('/api/status')['peers'] == 0

            # Spans posted to the remote on its own capture clock cross the
            # link and land in the hub's ring, on the hub's clock. On one box
            # the clocks are the same, so the shift is the sync error.
            now = remote('/api/events', {})['monotonic_now_ns']
            spans = [{'pid': 7001, 'tid': 7002, 'name': f'remote-span-{i}', 'start_ns': now - 50_000_000 + i * 1_000_000, 'duration_ns': 250_000}
                     for i in range(20)]
            summary = remote('/api/events', {'processes': [{'pid': 7001, 'name': 'pipeline'}],
                                             'threads': [{'pid': 7001, 'tid': 7002, 'name': 'stage'}], 'spans': spans})
            assert summary['accepted'] == 20, summary
            packed_7001 = (machine << MACHINE_SHIFT) | 7001

            def crossed():
                bundle = hub('/api/capture/export?format=bundle')
                with zipfile.ZipFile(io.BytesIO(bundle)) as capture:
                    manifest = json.loads(capture.read('manifest.json'))
                    rows = parquet.read_table(io.BytesIO(capture.read(manifest['files']['events']))).to_pylist()
                ours = [r for r in rows if r['pid'] == packed_7001]
                return (ours, manifest) if len(ours) >= 20 else None
            ours, manifest = wait_for('the spans to cross the link', crossed, 20, services)
            by_name = {r['name']: r for r in ours}
            errors = []
            for span in spans:
                row = by_name.get(span['name'])
                assert row, f"{span['name']} missing on the hub; got {sorted(by_name)}"
                assert row['tid'] == (machine << MACHINE_SHIFT) | 7002, row
                assert row['duration_ns'] == 250_000, row
                assert row['kind'] == KIND_API_SCOPE, row
                errors.append(row['start_ns'] - span['start_ns'])
            worst = max(abs(e) for e in errors)
            print(f'20 spans crossed the link; stamps moved by {min(errors)}..{max(errors)} ns (worst {worst} ns, band ±{args.band_us:.0f} µs)')
            assert worst < args.band_us * 1000, f'a span moved by {worst} ns on one box'
            processes = manifest['bundle']['processes']
            assert any(p['pid'] == packed_7001 and p['name'] == f'pipeline @{host}' for p in processes), processes
            threads = manifest['bundle']['threads']
            assert any(t['pid'] == packed_7001 and t['name'] == 'stage' for t in threads), threads

            # The remote's processes in the hub's list, and a Record on the
            # hub for one of them.
            target = subprocess.Popen([target_path, '--seconds', str(int(args.seconds) + 20)], stdout=subprocess.PIPE, stderr=remote_log, text=True, env=remote_env)
            read_line_within(target, 10)
            packed_target = (machine << MACHINE_SHIFT) | target.pid

            def listed():
                entry = next((p for p in hub('/api/processes') if p['pid'] == packed_target), None)
                return entry
            entry = wait_for("the target in the hub's process list", listed, 20, services)
            assert entry['name'].endswith(f' @{host}'), entry
            assert entry['machine'] == host, entry
            print(f"hub lists {entry['name']!r} as pid {packed_target:#x}")

            hub('/api/capture/start', {'pid': packed_target})
            wait_for('the remote to start capturing', lambda: remote('/api/status')['capturing'], 10, services)
            status = hub('/api/status')
            assert status['capturing'] and status['target_pid'] == packed_target, status
            time.sleep(args.seconds)
            hub('/api/capture/stop', {})
            wait_for('the remote to stop capturing', lambda: not remote('/api/status')['capturing'], 10, services)
            assert not hub('/api/status')['capturing']
            bundle = hub('/api/capture/export?format=bundle')
            with zipfile.ZipFile(io.BytesIO(bundle)) as capture:
                manifest = json.loads(capture.read('manifest.json'))
                rows = parquet.read_table(io.BytesIO(capture.read(manifest['files']['events']))).to_pylist()
            scopes = [r for r in rows if r['pid'] == packed_target and r['kind'] == KIND_API_SCOPE]
            names = {r['name'] for r in scopes}
            print(f"{len(scopes)} scopes of the remote's target in the hub's capture: {sorted(names)[:6]}")
            assert len(scopes) > 50, 'the remote target\'s scopes did not reach the hub'
            assert all(r['tid'] >> MACHINE_SHIFT == machine for r in scopes), 'a thread of the remote target kept a bare tid'
            print(f'PASS: one timeline from two services, {host} joined as machine {machine}, alignment within {worst / 1000:.1f} µs')
        except BaseException:
            for name, log in (('hub', hub_log), ('remote', remote_log)):
                log.seek(0)
                print(f'--- {name} log', file=sys.stderr)
                print(log.read(), file=sys.stderr)
            raise
        finally:
            for child in (target, remote_proc, hub_proc):
                if child is not None and child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()


if __name__ == '__main__':
    main()
