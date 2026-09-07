#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Attach to a running target; verify counts, nesting, manual coexistence and restart.
Requires frida==17.17.0 and pyarrow in the selected Python environment.
"""
import argparse
import io
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import zipfile
import pyarrow.parquet as parquet


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--service', required=True)
    parser.add_argument('--agent', required=True)
    parser.add_argument('--target', required=True)
    parser.add_argument('--engine', default='frida', choices=['frida', 'kernel_uprobes'])
    parser.add_argument('--missing-agent', action='store_true')
    args = parser.parse_args()
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    def request(path, body=None):
        req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(),
                                     headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=60) as response:
            data = response.read()
            return json.loads(data) if data.startswith((b'{', b'[')) else data
    with tempfile.TemporaryFile(mode='w+') as log:
        env = dict(os.environ, ORBIT_FRIDA_AGENT=str(Path(args.agent).resolve()) + ('.missing' if args.missing_agent else ''), ORBIT_FRIDA_PYTHON=os.sys.executable)
        service = subprocess.Popen([args.service, '--host', '127.0.0.1', '--serve', str(port)], env=env, stdout=log, stderr=log)
        target = subprocess.Popen([args.target], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            ready, _, _ = select.select([target.stdout], [], [], 10)
            assert ready
            pid = int(target.stdout.readline())
            deadline = time.monotonic() + 30
            while True:
                try:
                    request('/api/status'); break
                except OSError:
                    assert service.poll() is None and time.monotonic() < deadline
                    time.sleep(.1)
            if args.engine == 'frida' and not args.missing_agent:
                import frida
                # Exercise Mach-O address/offset arithmetic on every host using
                # Frida's actual UInt64 and NativePointer types.
                probe = frida.attach(pid)
                source = (Path(__file__).resolve().parents[1] / 'frida/agent.js').read_text()
                check = probe.create_script(source + """
                rpc.exports.checkSegments = function () {
                    const base = Memory.alloc(8192);
                    base.writeU32(0xfeedfacf); base.add(16).writeU32(2);
                    for (let i = 0; i < 2; i++) {
                        const c = base.add(32 + i * 72);
                        c.writeU32(0x19); c.add(4).writeU32(72);
                        c.add(24).writeU64(uint64('0x100000000').add(i * 4096));
                        c.add(40).writeU64(i * 4096); c.add(48).writeU64(4096);
                        c.add(60).writeU32(i === 0 ? 5 : 3);
                    }
                    const s = segments({base:base});
                    return s.length === 2 && s[0].address.equals(base) &&
                        s[1].address.equals(base.add(4096)) && s[0].executable && !s[1].executable;
                };
                """)
                try:
                    check.load()
                    assert check.exports_sync.check_segments(), 'Mach-O segment translation failed'
                finally:
                    probe.detach()
            request('/api/symbols/load', {'pid': pid})
            while True:
                state = request(f'/api/symbols/status?pid={pid}')
                if state['status'] == 'ready': break
                assert state['status'] != 'error', state
                assert time.monotonic() < deadline, state
                time.sleep(.1)
            found = request(f'/api/functions/search?pid={pid}&q=orbit_frida_test_&limit=20')['functions']
            wanted = {'orbit_frida_test_outer': (30, 0), 'orbit_frida_test_middle': (60, 1), 'orbit_frida_test_inner': (180, 2)}
            hooks = [f for f in found if f['name'].lstrip('_') in wanted]
            assert len(hooks) == 3, found
            if args.missing_agent:
                try:
                    request('/api/capture/start', {'pid':pid, 'instrumented_functions':[{'function_id': hooks[0]['function_id']}]})
                except urllib.error.HTTPError as error:
                    assert b'Frida agent' in error.read()
                else:
                    raise AssertionError('missing Frida runtime silently accepted')
                request('/api/capture/start', {'pid':pid})
                request('/api/capture/stop', {})
                print('PASS: missing Frida runtime fails Start; manual capture still starts afterward')
                return
            for iteration in range(2):
                # Omit the method on the first run to exercise the actual default.
                body = {'pid':pid, 'instrumented_functions':[{'function_id': f['function_id']} for f in hooks]}
                if iteration or args.engine != 'frida': body['dynamic_instrumentation_method'] = args.engine
                request('/api/capture/start', body)
                time.sleep(.3)  # Allow late manual segment discovery.
                target.stdin.write('go\n'); target.stdin.flush()
                ready, _, _ = select.select([target.stdout], [], [], 20)
                assert ready and target.stdout.readline().strip() == 'done', 'target stalled/crashed'
                # Uprobes retain records briefly to reorder cross-CPU events.
                time.sleep(.15)
                request('/api/capture/stop', {})
                status = request('/api/status')
                if args.engine == 'frida': assert status['instrumentation'].startswith('Frida:'), status
                with zipfile.ZipFile(io.BytesIO(request('/api/capture/export?format=bundle'))) as capture:
                    manifest = json.loads(capture.read('manifest.json'))
                    rows = parquet.read_table(io.BytesIO(capture.read(manifest['files']['events']))).to_pylist()
                for name, (count, depth) in wanted.items():
                    events = [r for r in rows if r['pid'] == pid and r['name'].lstrip('_') == name]
                    assert len(events) == count, (iteration, name, len(events), status['instrumentation'])
                    assert all(e['depth'] == depth and e['duration_ns'] > 0 for e in events), (name, events[:3])
                    tids = {e['tid'] for e in events}
                    assert len(tids) == 3
                    assert tids == {r['tid'] for r in rows if r['pid'] == pid and r['name'] == 'manual worker'}, 'manual and dynamic thread identities differ'
                assert any(r['pid'] == pid and r['name'] == 'manual alongside Frida' for r in rows), 'manual segment was replaced'
                assert target.poll() is None, 'detach killed target'
            print(f'PASS {args.engine}: 270 exact spans per capture, three threads, correct depths, manual coexistence, restart')
        except BaseException:
            log.seek(0); print(log.read()); raise
        finally:
            for process in [target, service]:
                process.terminate()
                try: process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait()

if __name__ == '__main__': main()
