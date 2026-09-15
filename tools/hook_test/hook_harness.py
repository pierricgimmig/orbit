#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Rock-solid-hooking harness: instrument one function at a time and check the
event arrives, so a hook that silently does nothing -- or crashes the target --
is caught function by function.

For each process in the config (`tools/hook_test/targets.toml`), it takes a
list of functions (the hottest from a sampling capture, a name search, or an
explicit list), then for each function: arms exactly that one hook, lets the
target run, and reads how many of its calls were recorded. A target that dies
while a single function is hooked names its own culprit; the exit signal comes
from the launcher, and `gdb` (opt-in) adds a backtrace.

Run:
    python3 tools/hook_test/hook_harness.py --sudo
    python3 tools/hook_test/hook_harness.py --sudo --only workload
    python3 tools/hook_test/hook_harness.py --sudo --config my_targets.toml

`--sudo` runs the service through the passwordless wrapper (uprobes need
CAP_SYS_ADMIN); without it, the run reports the arming refusal and stops.
Add a process by editing the TOML -- a demo you build or a released game.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(REPO, "tools/e2e"))
import orbit_e2e as e2e  # noqa: E402  (path set above)

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover
    tomllib = None


# ------------------------------------------------------------------- launching


class Launched:
    """A launched target: its pid, a way to send it the go line, whether it is
    still alive and how it died, and any gdb backtrace."""

    def __init__(self, argv, use_gdb):
        self.argv = argv
        self.use_gdb = use_gdb
        self.backtrace = ""
        if use_gdb:
            # gdb runs the inferior; `run` blocks until it stops or exits, then
            # `bt` prints the backtrace. The inferior's stdout (its `pid=` line)
            # passes through, and its stdin is gdb's stdin, so `send()` reaches
            # a program waiting on a line.
            cmd = ["gdb", "-q", "-batch", "-ex", "set pagination off",
                   "-ex", "run", "-ex", "bt", "-ex", "kill", "-ex", "quit", "--args", *argv]
            self.proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                         stderr=subprocess.STDOUT, text=True)
        else:
            self.proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                         stderr=subprocess.DEVNULL, text=True)
        self.pid = self._read_pid()

    def _read_pid(self):
        # Our demos print `pid=<n>`; a released binary may not, so fall back to
        # the launched pid (the inferior under gdb is a child, so gdb's pid is
        # wrong there -- such targets should print their pid).
        deadline = time.time() + 10
        while time.time() < deadline:
            line = self.proc.stdout.readline()
            if not line:
                break
            m = re.search(r"pid=(\d+)", line)
            if m:
                return int(m.group(1))
            if "[Inferior" in line or "Starting program" in line:
                continue
        if self.use_gdb:
            raise e2e.Failure("a gdb target must print `pid=<n>` on its first stdout line")
        return self.proc.pid

    def send(self, text):
        try:
            self.proc.stdin.write(text)
            self.proc.stdin.flush()
        except (BrokenPipeError, ValueError):
            pass

    def alive(self):
        return self.proc.poll() is None and os.path.exists(f"/proc/{self.pid}")

    def signal(self):
        """The termination signal name if it died by one, else ''. Under gdb
        the exit code is gdb's, so death is read from the backtrace instead."""
        code = self.proc.poll()
        if code is not None and code < 0:
            try:
                import signal as _sig
                return _sig.Signals(-code).name
            except ValueError:
                return f"signal {-code}"
        return ""

    def finish_gdb(self, timeout=15.0):
        """Drain a gdb inferior to completion and read the outcome from gdb's
        output: whether it crashed, the signal, and the backtrace. gdb holds a
        faulting inferior in trace-stop, so its state cannot be read from
        /proc -- gdb's own report is the truth."""
        try:
            out, _ = self.proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            out, _ = self.proc.communicate()
        out = out or ""
        m = re.search(r"received signal (SIG\w+)", out)
        signal = m.group(1) if m else ""
        lines = [ln for ln in out.splitlines()
                 if ln.startswith("#") or "received signal" in ln or "Program terminated" in ln]
        self.backtrace = "\n".join(lines[:24])
        return bool(signal), signal, self.backtrace

    def kill(self):
        try:
            self.proc.kill()
            self.proc.wait(timeout=3)
        except Exception:  # noqa: BLE001
            pass


# --------------------------------------------------------------------- picking


def pick_functions(service, pid, select, top):
    """Resolve the config's `select` into (function_id, name, safety) rows."""
    kind, _, arg = select.partition(":")
    if kind == "report":
        # A short sampling capture, then the hottest functions with an id.
        service.post("/api/capture/start", {"pid": pid, "sampling": True})
        time.sleep(3.0)
        service.post("/api/capture/stop")
        report = service.get("/api/sampling/report?start_ns=0&end_ns=18446744073709551615")
        rows = [r for r in report.get("functions", []) if r.get("function_id")]
        rows = rows[:top]
        return [(r["function_id"], r["name"], "") for r in rows]
    if kind == "search":
        hits = service.get(f"/api/functions/search?pid={pid}&q={arg}&limit={top}")["functions"]
        return [(h["function_id"], h["name"], h.get("safety", "")) for h in hits]
    if kind == "list":
        out = []
        for name in [n.strip() for n in arg.split(",") if n.strip()]:
            hits = service.get(f"/api/functions/search?pid={pid}&q={name}&limit=8")["functions"]
            exact = [h for h in hits if h["name"] == name] or hits[:1]
            if exact:
                out.append((exact[0]["function_id"], exact[0]["name"], exact[0].get("safety", "")))
        return out
    raise e2e.Failure(f"unknown select {select!r} (use report / search:<q> / list:a,b)")


def load_symbols(service, pid, timeout=60):
    service.post("/api/symbols/load", {"pid": pid})
    deadline = time.time() + timeout
    while time.time() < deadline:
        st = service.get(f"/api/symbols/status?pid={pid}")
        if st.get("status") == "ready":
            return True
        if st.get("status") == "error":
            return False
        time.sleep(0.3)
    return False


def calls_recorded(service):
    """The single hooked function's completed calls, from the service log line
    the capture stop prints ("N instrumented calls recorded"). With exactly one
    hook armed, that count is this function's."""
    try:
        service.log.flush()
        text = open(service.log.name).read()
    except OSError:
        return None
    hits = re.findall(r"(\d+) instrumented calls recorded", text)
    return int(hits[-1]) if hits else None


# ------------------------------------------------------------------ one target


def test_target(service, target, seconds_default):
    name = target["name"]
    argv = [a.replace("$BUILT", target.get("out", "")) for a in target["argv"]]
    argv[0] = argv[0] if os.path.isabs(argv[0]) else os.path.join(REPO, argv[0])
    select = target.get("select", "report")
    top = int(target.get("top", 15))
    seconds = float(target.get("seconds", seconds_default))
    wait_go = bool(target.get("wait_go", False))
    use_gdb = bool(target.get("gdb", False))

    results = []

    def launch():
        return Launched(argv, use_gdb)

    app = launch()
    print(f"  {name}: pid {app.pid}  ({' '.join(argv)})")
    if not load_symbols(service, app.pid):
        app.kill()
        return [{"function": "(symbols)", "outcome": "error", "detail": "symbols did not load"}]
    functions = pick_functions(service, app.pid, select, top)
    if not functions:
        app.kill()
        return [{"function": "(select)", "outcome": "error", "detail": f"no functions from {select!r}"}]

    for fid, fname, safety in functions:
        # A gdb target is single-shot (gdb's `run` ends at the crash), and a
        # crashed non-gdb target is gone -- either way, start fresh when the
        # current process is not usable.
        if app is None or not app.alive():
            app = launch()
            load_symbols(service, app.pid)
        # Arm exactly this one function.
        service.post("/api/capture/start", {
            "pid": app.pid,
            "instrumented_functions": [{"function_id": fid}],
            "dynamic_instrumentation_method": "kernel_uprobes",
            "sampling": False,
        })
        message = ""
        deadline = time.time() + 10
        while time.time() < deadline:
            message = service.get("/api/status").get("instrumentation", "")
            if message:
                break
            time.sleep(0.2)
        if "no hooks armed" in message:
            service.post("/api/capture/stop")
            app.kill()
            raise e2e.Failure(f"uprobes not permitted: {message.split('.')[0]} (run with --sudo)")
        # Let it run; a wait-go target does its work (and any crash) now.
        if wait_go:
            app.send("go\n")

        if use_gdb:
            # gdb blocks in `run` until the inferior stops; read the outcome
            # from gdb, then this process is spent.
            crashed, sig, bt = app.finish_gdb()
            service.post("/api/capture/stop")
            app = None
            if crashed:
                results.append({"function": fname, "safety": safety, "outcome": "crash",
                                "signal": sig, "backtrace": bt,
                                "detail": f"the target died ({sig}) while only {fname} was hooked"})
                print(f"    CRASH  {fname}  ({sig})")
            else:
                calls = calls_recorded(service)
                outcome = "received" if calls else "no-events"
                results.append({"function": fname, "safety": safety, "outcome": outcome, "calls": calls or 0})
                print(f"    {'ok    ' if calls else 'armed '} {fname}  ({calls or 0} calls, exited)")
            continue

        time.sleep(seconds)
        crashed = not app.alive()
        service.post("/api/capture/stop")
        if crashed:
            sig = app.signal()
            detail = f"the target died ({sig or 'terminated'}) while only {fname} was hooked"
            results.append({"function": fname, "safety": safety, "outcome": "crash",
                            "signal": sig, "detail": detail, "backtrace": ""})
            print(f"    CRASH  {fname}  ({sig or 'terminated'})")
            app = None  # relaunch for the next function
            continue
        calls = calls_recorded(service)
        if calls and calls > 0:
            results.append({"function": fname, "safety": safety, "outcome": "received", "calls": calls})
            print(f"    ok     {fname}  {calls} calls")
        else:
            results.append({"function": fname, "safety": safety, "outcome": "no-events",
                            "detail": "armed, but no calls were recorded (not exercised in the window)"})
            print(f"    armed  {fname}  (no calls in {seconds:.0f}s)")
    if app is not None:
        app.kill()
    return results


# -------------------------------------------------------------------- the run


def build_target(target):
    cmd = target.get("build")
    out = target.get("out")
    if not cmd:
        return
    if out and os.path.exists(out):
        return
    print(f"  building {target['name']}: {cmd}")
    subprocess.run(cmd, shell=True, cwd=REPO, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--config", default=os.path.join(HERE, "targets.toml"))
    parser.add_argument("--only", action="append", help="run only these target names")
    parser.add_argument("--sudo", action="store_true", help="run the service privileged (uprobes)")
    parser.add_argument("--port", type=int, default=44820)
    parser.add_argument("--seconds", type=float, default=3.0, help="capture seconds per function")
    parser.add_argument("--report", default=os.path.join(REPO, "docs/hook-test/report.md"))
    args = parser.parse_args()

    if tomllib is None:
        print("need Python 3.11+ for tomllib", file=sys.stderr)
        return 2
    with open(args.config, "rb") as handle:
        config = tomllib.load(handle)
    targets = config.get("target", [])
    if args.only:
        targets = [t for t in targets if t["name"] in args.only]
    if not targets:
        print(f"no targets match {args.only}", file=sys.stderr)
        return 2

    e2e.SUDO = args.sudo
    service = e2e.Service(args.port)
    print(f"service on {service.base} (sudo={args.sudo})")
    all_results = {}
    try:
        for target in targets:
            print(f"target {target['name']}")
            try:
                build_target(target)
                all_results[target["name"]] = test_target(service, target, args.seconds)
            except Exception as error:  # noqa: BLE001 - reported per target
                all_results[target["name"]] = [{"function": "(target)", "outcome": "error", "detail": str(error)}]
                print(f"  ERROR {target['name']}: {error}")
    finally:
        service.stop()

    # Summary + report.
    os.makedirs(os.path.dirname(args.report), exist_ok=True)
    lines = ["# Hook test report", ""]
    totals = {"received": 0, "no-events": 0, "crash": 0, "error": 0}
    for name, rows in all_results.items():
        lines.append(f"## {name}")
        lines.append("")
        lines.append("| function | outcome | detail |")
        lines.append("|---|---|---|")
        for r in rows:
            totals[r["outcome"]] = totals.get(r["outcome"], 0) + 1
            detail = r.get("detail", "") or (f"{r.get('calls')} calls" if r.get("calls") else "")
            lines.append(f"| `{r['function']}` | {r['outcome']} | {detail} |")
            if r.get("backtrace"):
                lines.append(f"| | | <pre>{r['backtrace'].replace(chr(10), '<br>')}</pre> |")
        lines.append("")
    with open(args.report, "w") as handle:
        handle.write("\n".join(lines))
    with open(args.report.replace(".md", ".json"), "w") as handle:
        json.dump(all_results, handle, indent=2)

    print()
    print(f"received {totals['received']}, no-events {totals['no-events']}, "
          f"crashes {totals['crash']}, errors {totals['error']}")
    print(f"report: {args.report}")
    return 1 if totals["crash"] or totals["error"] else 0


if __name__ == "__main__":
    sys.exit(main())
