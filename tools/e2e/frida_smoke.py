#!/usr/bin/env python3
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Attach to a running target; verify counts, nesting, manual coexistence and restart.
Requires pyarrow for capture assertions. Frida injection uses the native helper.
"""
import argparse
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import zipfile
import pyarrow.parquet as parquet


def read_line_within(child, seconds):
    """One line of the child's stdout, or None after `seconds`: a reader
    thread, because `select` on a pipe is POSIX-only and this runs on
    Windows too."""
    import threading
    box = []
    reader = threading.Thread(target=lambda: box.append(child.stdout.readline()), daemon=True)
    reader.start()
    reader.join(seconds)
    return box[0].strip() if box else None


def helper_children(service_pid):
    """The pids of perfr-frida-helper processes the service spawned."""
    if sys.platform == 'win32':
        script = (f"Get-CimInstance Win32_Process -Filter 'ParentProcessId={service_pid}' | "
                  "Where-Object { $_.Name -like 'perfr-frida-helper*' } | ForEach-Object { $_.ProcessId }")
        out = subprocess.check_output(['powershell', '-NoProfile', '-Command', script], text=True)
        return [int(line) for line in out.split() if line.strip().isdecimal()]
    processes = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,args='], text=True)
    return [int(fields[0]) for row in processes.splitlines()
            if len(fields := row.split(None, 2)) == 3
            and int(fields[1]) == service_pid and 'perfr-frida-helper' in fields[2]]


def kill_hard(pid):
    if sys.platform == 'win32':
        subprocess.run(['taskkill', '/F', '/PID', str(pid)], check=True, capture_output=True)
    else:
        import signal
        os.kill(pid, signal.SIGKILL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--service', required=True)
    parser.add_argument('--agent', required=True)
    parser.add_argument('--target', required=True)
    parser.add_argument('--helper', required=True)
    parser.add_argument('--engine', default='frida', choices=['frida', 'kernel_uprobes'])
    parser.add_argument('--missing-agent', action='store_true')
    parser.add_argument('--missing-helper', action='store_true')
    parser.add_argument('--no-manual', action='store_true')
    parser.add_argument('--inflight', action='store_true')
    parser.add_argument('--controller-death', action='store_true')
    args = parser.parse_args()
    # Absolute, native paths: a relative path with forward slashes is not
    # something Windows process creation resolves.
    for name in ('service', 'agent', 'target', 'helper'):
        setattr(args, name, str(Path(getattr(args, name)).resolve()))
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
    with tempfile.TemporaryFile(mode='w+') as log:
        env = dict(os.environ, PERFR_FRIDA_AGENT=str(Path(args.agent).resolve()) + ('.missing' if args.missing_agent else ''), PERFR_FRIDA_HELPER=str(Path(args.helper).resolve()) + ('.missing' if args.missing_helper else ''))
        service = subprocess.Popen([args.service, '--host', '127.0.0.1', '--serve', str(port)], env=env, stdout=log, stderr=log)
        target = subprocess.Popen([args.target], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        try:
            announcement = read_line_within(target, 10)
            assert announcement is not None, 'target did not announce its PID within 10 seconds'
            assert announcement.isdecimal(), (
                f'target failed before announcing its PID: exit={target.poll()}, stdout={announcement!r}')
            pid = int(announcement)
            deadline = time.monotonic() + 30
            while True:
                # A short timeout: a probe sent before the service listens can
                # be dropped rather than refused (Windows), and a long one would
                # then sit on SYN retries instead of trying a fresh socket.
                try:
                    request('/api/status', timeout=1); break
                except OSError:
                    assert service.poll() is None and time.monotonic() < deadline
                    time.sleep(.1)
            request('/api/symbols/load', {'pid': pid})
            while True:
                state = request(f'/api/symbols/status?pid={pid}')
                if state['status'] == 'ready': break
                assert state['status'] != 'error', state
                assert time.monotonic() < deadline, state
                time.sleep(.1)
            found = request(f'/api/functions/search?pid={pid}&q=perfr_frida_test_&limit=20')['functions']
            wanted = {'perfr_frida_test_outer': (30, 0), 'perfr_frida_test_middle': (60, 1), 'perfr_frida_test_inner': (180, 2)}
            # A PDB names a C function with its (empty) argument list, as a
            # demangled C++ name would; the bare name is what is compared.
            bare = lambda name: name.lstrip('_').split('(')[0]
            hooks = [f for f in found if bare(f['name']) in wanted]
            assert len(hooks) == 3, found
            if args.missing_agent or args.missing_helper:
                try:
                    request('/api/capture/start', {'pid':pid, 'instrumented_functions':[{'function_id': hooks[0]['function_id']}]})
                except urllib.error.HTTPError as error:
                    message = error.read()
                    expected = b'Frida helper' if args.missing_helper else b'Frida agent'
                    assert expected in message, message
                else:
                    raise AssertionError('missing Frida runtime silently accepted')
                request('/api/capture/start', {'pid':pid})
                request('/api/capture/stop', {})
                print('PASS: missing Frida runtime fails Start; manual capture still starts afterward')
                return
            pending_return = False
            if args.inflight or args.controller_death:
                blocked = next(f for f in found if bare(f['name']) == 'perfr_frida_test_blocked')
                request('/api/capture/start', {'pid':pid, 'instrumented_functions':[{'function_id':blocked['function_id']}]})
                target.stdin.write('hold\n'); target.stdin.flush()
                assert read_line_within(target, 10) == 'entered', 'blocked hook did not enter'
                if args.controller_death:
                    children = helper_children(service.pid)
                    assert len(children) == 1, children
                    kill_hard(children[0])
                    time.sleep(.3)  # EOF cleanup in the target; no helper destructor.
                started = time.monotonic()
                request('/api/capture/stop', {})
                assert time.monotonic() - started < 5, 'Stop waited for target return'
                pending_return = True
            mixed = args.engine == 'frida' and not args.no_manual
            if mixed: wanted = {name: (count, depth * 2 + 1) for name, (count, depth) in wanted.items()}
            for iteration in range(2):
                # Omit the method on the first run to exercise the actual default.
                body = {'pid':pid, 'instrumented_functions':[{'function_id': f['function_id']} for f in hooks]}
                if iteration or args.engine != 'frida': body['dynamic_instrumentation_method'] = args.engine
                request('/api/capture/start', body)
                if args.engine == 'frida' and iteration == 0:
                    # Read-only discovery must not steal or reject the active
                    # capture's controller lease.
                    # Exercise short-lived Core injectors repeatedly: teardown
                    # failures may only appear after an otherwise valid reply.
                    for _ in range(20 if args.no_manual else 1):
                        probe = subprocess.run([args.helper], input=json.dumps({
                            'pid':pid, 'agent':str(Path(args.agent).resolve()), 'command':'symbols'
                        }) + '\n', text=True, capture_output=True, timeout=30)
                        assert probe.returncode == 0, (probe.returncode, probe.stdout[-2048:], probe.stderr[-4096:])
                        reply = json.loads(probe.stdout.splitlines()[0])
                        assert 'symbols' in reply, reply
                        discovered = {s['name'].lstrip('_') for s in reply['symbols']}
                        # Gum's symbol enumeration is what the macOS function
                        # index reads; Windows reads PDBs in the service and
                        # Gum reports nothing there, so only the lease is
                        # checked (the probe returned, without stealing it).
                        if sys.platform != 'win32':
                            assert set(wanted) <= discovered, (set(wanted) - discovered, len(discovered))
                if pending_return:
                    # The old leave listener must survive detach and reject its
                    # old generation after a new capture has opened.
                    target.stdin.write('release\n'); target.stdin.flush()
                    assert read_line_within(target, 10) == 'released', 'late return crashed/stalled'
                    pending_return = False
                time.sleep(.3)  # Allow late manual segment discovery.
                target.stdin.write('go\n'); target.stdin.flush()
                assert read_line_within(target, 20) == 'done', f'target stalled/crashed (exit={target.poll()})'
                # Uprobes retain records briefly to reorder cross-CPU events.
                time.sleep(.15)
                request('/api/capture/stop', {})
                status = request('/api/status')
                if args.engine == 'frida': assert status['instrumentation'].startswith('Frida:'), status
                with zipfile.ZipFile(io.BytesIO(request('/api/capture/export?format=bundle'))) as capture:
                    manifest = json.loads(capture.read('manifest.json'))
                    rows = parquet.read_table(io.BytesIO(capture.read(manifest['files']['events']))).to_pylist()
                if args.engine == 'frida':
                    phases = [r for r in rows if r['pid'] == service.pid and r['name'].startswith('Frida: ')]
                    expected_phases = ['Frida: inject native agent', 'Frida: initialize Gum',
                                       'Frida: connect scope API', 'Frida: resolve executable address',
                                       'Frida: detach trampolines']
                    # The detach span is emitted at teardown and relayed over the
                    # control channel. A Windows named pipe has no half-close, so
                    # it is already closing when the agent detaches and the span
                    # does not round-trip; the detach still happens (hooks are
                    # removed, below). See docs/building_windows.md.
                    if sys.platform == 'win32':
                        expected_phases.remove('Frida: detach trampolines')
                    for name in expected_phases:
                        # Cached initialization can finish within a host clock tick
                        # (notably CLOCK_MONOTONIC on Darwin). Zero is valid.
                        assert any(r['name'] == name and r['duration_ns'] >= 0 for r in phases), ('missing self-profile phase', name)
                    installs = [r for r in phases if r['name'].startswith('Frida: install trampoline: ')]
                    assert len(installs) == len(wanted), ('missing hook installation timings', installs)
                    assert all(r['duration_ns'] > 0 and not (r['flags'] & 128) for r in installs)
                    arm = next(r for r in phases if r['name'] == 'Frida: arm hooks')
                    assert all(arm['start_ns'] <= r['start_ns'] and
                               r['start_ns'] + r['duration_ns'] <= arm['start_ns'] + arm['duration_ns']
                               for r in installs), 'remote timestamps do not align with service setup'

                assert not any(r['pid'] == pid and bare(r['name']) == 'perfr_frida_test_blocked' for r in rows), 'late return leaked into next capture'
                for name, (count, depth) in wanted.items():
                    events = [r for r in rows if r['pid'] == pid and r['kind'] == 1 and bare(r['name']) == name]
                    assert len(events) == count, (iteration, name, len(events), status['instrumentation'])
                    assert all(e['depth'] == depth and e['duration_ns'] > 0 for e in events), (name, events[:3])
                    tids = {e['tid'] for e in events}
                    assert len(tids) == 3
                    if not args.no_manual: assert tids == {r['tid'] for r in rows if r['pid'] == pid and r['name'] == 'manual worker'}, 'manual and dynamic thread identities differ'
                if not args.no_manual: assert any(r['pid'] == pid and r['name'] == 'manual alongside Frida' for r in rows), 'manual segment was replaced'
                if args.engine in ('frida', 'kernel_uprobes'):
                    dynamic = [r for r in rows if r['pid'] == pid and r['kind'] == 1 and bare(r['name']) in wanted]
                    assert all(r['flags'] & 128 for r in dynamic), 'dynamic provenance lost in export'
                if mixed:
                    expected = {'manual worker': (3, 0), 'manual outer': (30, 2),
                                'manual middle': (60, 4), 'manual inner': (180, 6), 'async worker': (3, 0)}
                    for name, (count, depth) in expected.items():
                        scopes = [r for r in rows if r['pid'] == pid and r['name'] == name]
                        assert len(scopes) == count, (name, len(scopes))
                        assert all(r['depth'] == depth and not (r['flags'] & 128) for r in scopes), (name, scopes[:3])
                    # Every synchronous child sits inside a parent on the same thread.
                    sync = [r for r in rows if r['pid'] == pid and r['name'] in expected and r['name'] != 'async worker'] + dynamic
                    for child in sync:
                        if child['depth'] == 0: continue
                        assert any(parent['tid'] == child['tid'] and parent['depth'] + 1 == child['depth']
                                   and parent['start_ns'] <= child['start_ns']
                                   and parent['start_ns'] + parent['duration_ns'] >= child['start_ns'] + child['duration_ns']
                                   for parent in sync), ('missing enclosing parent', child)
                assert target.poll() is None, 'detach killed target'
            print(f'PASS {args.engine}: 270 exact spans per capture, three threads, correct depths, manual/dynamic nesting and provenance, restart')
        except BaseException as error:
            if isinstance(error, urllib.error.HTTPError): print(error.read().decode())
            log.seek(0); print(log.read()); raise
        finally:
            for process in [target, service]:
                process.terminate()
                try: process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait()

if __name__ == '__main__': main()
