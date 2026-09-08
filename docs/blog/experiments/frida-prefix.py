#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Isolated Linux x86-64 Gum relocation probes for blog post 21.
Requires cc and frida==17.17.0. Each hook runs in a disposable child; the
back-edge example intentionally exercises code that may crash after attachment.
No Orbit agent is involved: this isolates Gum's default Interceptor behavior.
Run: python frida-prefix.py > ../metrics/frida-prefix.json
"""
import json
import platform
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import frida

C = r'''
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
extern int rip_load(int);
extern int interior_backedge(int);
extern int entry_backedge(int);
int main(int argc, char **argv) {
  struct rlimit core = {0, 0}; setrlimit(RLIMIT_CORE, &core);
  int (*f)(int) = strcmp(argv[1], "rip_load") == 0 ? rip_load :
    strcmp(argv[1], "interior_backedge") == 0 ? interior_backedge : entry_backedge;
  printf("%d %p\n", f(3), (void *) f); fflush(stdout);
  getchar();
  printf("%d\n", f(3)); fflush(stdout);
  getchar();
  return 0;
}
'''
ASM = r'''
.text
.globl rip_load
.type rip_load,@function
rip_load:
  mov datum(%rip), %eax
  ret
.size rip_load, .-rip_load
.p2align 4
.globl interior_backedge
.type interior_backedge,@function
interior_backedge:
  xor %eax, %eax
.Lloop:
  inc %eax
  cmp %edi, %eax
  jl .Lloop
  ret
.size interior_backedge, .-interior_backedge
.p2align 4
.globl entry_backedge
.type entry_backedge,@function
entry_backedge:
  .rept 5
  nop
  .endr
  sub $1, %edi
  jg entry_backedge
  mov %edi, %eax
  ret
.size entry_backedge, .-entry_backedge
.data
.p2align 2
datum: .long 42
.section .note.GNU-stack,"",@progbits
'''
JS = r'''
let listener, enter = 0, leave = 0;
rpc.exports = {
  arm(address) {
    const p = ptr(address);
    const bytes = () => Array.from(new Uint8Array(p.readByteArray(16)))
      .map(x => x.toString(16).padStart(2, '0')).join(' ');
    const before = bytes();
    listener = Interceptor.attach(p, {
      onEnter() { enter++; }, onLeave() { leave++; }
    });
    Interceptor.flush();
    return {before, after:bytes()};
  },
  counts() { return {enter, leave}; }
};
'''

def line(process):
    ready, _, _ = select.select([process.stdout], [], [], 4)
    if not ready:
        raise TimeoutError('child did not reply in four seconds')
    return process.stdout.readline().strip()


def run(binary, name):
    p = subprocess.Popen([str(binary), name], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    session = None
    result = {'case': name}
    try:
        baseline, address = line(p).split()
        result['baseline'] = int(baseline)
        session = frida.attach(p.pid)
        script = session.create_script(JS)
        script.load()
        try:
            result['patch'] = script.exports_sync.arm(address)
            result['attach'] = 'accepted'
        except frida.RPCException as error:
            result['attach'] = 'rejected'
            result['error'] = str(error)
            return result
        p.stdin.write('\n'); p.stdin.flush()
        reply = line(p)
        if reply:
            result['instrumented'] = int(reply)
            result['callbacks'] = script.exports_sync.counts()
            p.stdin.write('\n'); p.stdin.flush()
        p.wait(timeout=4)
        result['exit_code'] = p.returncode
        if p.returncode < 0:
            result['signal'] = signal.Signals(-p.returncode).name
    except (TimeoutError, subprocess.TimeoutExpired):
        result['outcome'] = 'timed out'
    finally:
        if p.poll() is None:
            p.kill()
        p.wait()
        if session is not None:
            try:
                session.detach()
            except frida.InvalidOperationError:
                pass
        p.stdin.close(); p.stdout.close(); p.stderr.close()
    return result


def main():
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise SystemExit('This assembly probe is specifically Linux x86-64.')
    if frida.__version__ != '17.17.0':
        raise SystemExit('Use frida==17.17.0 to reproduce the pinned experiment.')
    with tempfile.TemporaryDirectory(prefix='orbit-prefix-') as tmp:
        root = Path(tmp)
        (root / 'main.c').write_text(C)
        (root / 'functions.S').write_text(ASM)
        binary = root / 'probe'
        subprocess.run(['cc', '-O0', '-g', '-rdynamic', str(root / 'main.c'),
                        str(root / 'functions.S'), '-o', str(binary)], check=True)
        report = {'frida': frida.__version__, 'system': platform.system(),
                  'machine': platform.machine(), 'kernel': platform.release(),
                  'compiler': subprocess.check_output(['cc', '--version'], text=True).splitlines()[0],
                  'note': 'One isolated invocation per case; not a benchmark or an Orbit E2E test.',
                  'cases': [run(binary, name) for name in
                            ['rip_load', 'interior_backedge', 'entry_backedge']]}
        print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
