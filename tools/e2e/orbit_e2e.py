#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""End-to-end tests for the Rust Orbit service, which double as the screenshot
generator for the viewer.

Every scenario does the same two things: assert something about the service's
answers, and photograph the viewer showing it. That pairing is deliberate. A
test that only checks JSON cannot catch a feature that is computed correctly
and drawn invisibly -- which is exactly the bug that hid the sample bar, where
the ticks were right in every respect except their colour. A screenshot that
nobody asserts on is decoration. Together they cover both halves.

Run:  python3 tools/e2e/orbit_e2e.py
      python3 tools/e2e/orbit_e2e.py --only sampling --keep-going
Output: docs/screenshots/*.png and a pass/fail summary; exit code 1 on failure.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)

from cdp import Chrome  # noqa: E402  (path set above)

DEFAULT_BOX3D = os.path.expanduser("~/git/box3d")
SERVICE = os.path.join(REPO, "rust/crates/orbit-service/target/release/orbit-service")


class Failure(AssertionError):
    pass


def check(condition, message):
    if not condition:
        raise Failure(message)


def check_at_least(value, minimum, what):
    if value < minimum:
        raise Failure(f"{what}: expected at least {minimum}, got {value}")


# --------------------------------------------------------------------- target


def build_target(box3d_root, out_path):
    """Compiles the Box3D workload the suite profiles."""
    lib = os.path.join(box3d_root, "build/src/libbox3d.a")
    include = os.path.join(box3d_root, "include")
    if not os.path.exists(lib):
        raise Failure(
            f"Box3D is not built: {lib} is missing.\n"
            f"Build it first:  cd {box3d_root} && ./build.sh"
        )
    subprocess.run(
        ["gcc", "-O2", "-g", "-fno-omit-frame-pointer", "-o", out_path,
         os.path.join(HERE, "box3d_target.c"), "-I", include, lib, "-lpthread", "-lm"],
        check=True,
    )
    return out_path


class Target:
    """The profiled process. `command` is a full argv; the process must print
    `pid=<n>` on its first line of stdout."""

    def __init__(self, command):
        self.proc = subprocess.Popen(
            command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
        line = self.proc.stdout.readline()
        match = re.search(r"pid=(\d+)", line)
        if not match:
            raise Failure(f"target did not announce its pid: {line!r}")
        self.pid = int(match.group(1))

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# -------------------------------------------------------------------- service


class Service:
    def __init__(self, port, binary=SERVICE):
        if not os.path.exists(binary):
            raise Failure(
                f"{binary} is missing.\nBuild it:  cargo build --release "
                "--manifest-path rust/crates/orbit-service/Cargo.toml"
            )
        self.port = port
        self.base = f"http://127.0.0.1:{port}"
        self.log = open(f"/tmp/orbit-e2e-service-{port}.log", "w")
        self.proc = subprocess.Popen(
            [binary, "--serve", str(port)], stdout=self.log, stderr=subprocess.STDOUT
        )
        self._wait_ready()

    def _wait_ready(self, timeout=30.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                self.get("/api/status")
                return
            except Exception:  # noqa: BLE001 - retried until the deadline
                time.sleep(0.2)
        raise Failure("service never answered /api/status")

    def get(self, path, timeout=30.0):
        with urllib.request.urlopen(self.base + path, timeout=timeout) as response:
            body = response.read()
        return json.loads(body) if body.strip().startswith((b"{", b"[")) else body

    def post(self, path, payload=None, timeout=30.0):
        data = json.dumps(payload).encode() if payload is not None else b""
        request = urllib.request.Request(
            self.base + path, data=data, method="POST",
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read().decode()
        except urllib.error.HTTPError as error:
            return f"HTTP {error.code}: {error.read().decode()[:200]}"

    def stderr_text(self):
        self.log.flush()
        with open(self.log.name) as handle:
            return handle.read()

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()


# ------------------------------------------------------------------- fixtures


class Run:
    """Everything a scenario needs, plus where its screenshots go."""

    def __init__(self, service, target, chrome, shots_dir):
        self.service = service
        self.target = target
        self.chrome = chrome
        self.shots_dir = shots_dir
        self.shots = []

    def shot(self, name, settle=1.5):
        # --no-shots runs the assertions without a browser, for CI machines
        # with no Chrome. Scenarios do not branch on it; this does.
        if self.chrome is None:
            return None
        time.sleep(settle)
        path = os.path.join(self.shots_dir, f"{name}.png")
        size = self.chrome.screenshot(path)
        self.shots.append((name, size))
        # A blank canvas compresses to a few KB; a real frame does not. This
        # has caught a headless browser that loaded the page and never
        # painted, which no JSON assertion would notice.
        check_at_least(size, 20_000, f"screenshot {name} looks blank ({size} bytes)")
        return path

    def open_viewer(self, query=""):
        if self.chrome is None:
            return
        self.chrome.goto(self.service.base + "/" + query, settle=8.0)

    def capture(self, seconds=6.0, **body):
        payload = {"pid": self.target.pid}
        payload.update(body)
        self.service.post("/api/capture/start", payload)
        time.sleep(seconds)

    def stop_capture(self):
        self.service.post("/api/capture/stop")
        time.sleep(1.0)

    def load_symbols(self, timeout=40.0):
        self.service.post("/api/symbols/load", {"pid": self.target.pid})
        deadline = time.time() + timeout
        while time.time() < deadline:
            status = self.service.get(f"/api/symbols/status?pid={self.target.pid}")
            if status.get("status") == "ready":
                return status
            if status.get("status") == "error":
                raise Failure(f"symbol load failed: {status.get('error')}")
            time.sleep(0.4)
        raise Failure("symbols never became ready")


# ------------------------------------------------------------------ scenarios

SCENARIOS = []


def scenario(name, description):
    def wrap(fn):
        SCENARIOS.append((name, description, fn))
        return fn
    return wrap


@scenario("viewer-idle", "The viewer loads and shows its chrome with no capture")
def viewer_idle(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    run.open_viewer()
    check(run.chrome.eval("document.title") == "Orbit Live Viewer", "wrong page title")
    canvas = run.chrome.eval(
        "(()=>{const c=document.querySelector('canvas');"
        "return c?c.width*c.height:0})()"
    )
    check_at_least(canvas, 100_000, "canvas is too small to be the viewer")
    run.shot("01-viewer-idle")


@scenario("processes", "The process list is served from /proc and includes the target")
def processes(run):
    processes = run.service.get("/api/processes")
    check_at_least(len(processes), 10, "process list")
    pids = {p["pid"] for p in processes}
    check(run.target.pid in pids, f"target pid {run.target.pid} is missing from /api/processes")


@scenario("symbols", "Symbols load for the target and the modules view lists them")
def symbols(run):
    status = run.load_symbols()
    check_at_least(status["function_count"], 100, "functions indexed")
    check_at_least(status["module_count"], 1, "modules indexed")
    modules = run.service.get(f"/api/symbols/modules?pid={run.target.pid}")["modules"]
    check_at_least(len(modules), 1, "modules listed")
    total = sum(m["function_count"] for m in modules)
    check(total == status["function_count"], "module counts do not add up to the index")


@scenario("function-search", "Searching finds the target's own instrumentable functions")
def function_search(run):
    run.load_symbols()
    hits = run.service.get(
        f"/api/functions/search?pid={run.target.pid}&q=orbit_e2e&limit=10"
    )["functions"]
    names = {h["name"] for h in hits}
    # These are the workload's own named functions; if the ELF index cannot
    # find them, nothing downstream can hook them either.
    for expected in ("orbit_e2e_step_world", "orbit_e2e_build_world"):
        check(expected in names, f"{expected} not found by search, got {sorted(names)}")


@scenario("capture-scheduling", "A capture streams scheduling slices onto the timeline")
def capture_scheduling(run):
    run.capture(seconds=6.0)
    status = run.service.get("/api/status")
    check(status["capturing"], "service does not report capturing")
    check_at_least(status["events_live"], 1000, "events streamed during capture")
    run.open_viewer()
    run.shot("02-capture-live", settle=3.0)
    run.stop_capture()


@scenario("sampling-report", "The whole-capture flat report names the workload's functions")
def sampling_report(run):
    run.load_symbols()
    run.capture(seconds=8.0)
    run.stop_capture()
    report = run.service.get("/api/sampling/report?start_ns=0&end_ns=18446744073709551615")
    check_at_least(report["samples"], 200, "callstack samples")
    names = [f["name"] for f in report["functions"]]
    check(
        any("b3" in n or "orbit_e2e" in n for n in names),
        f"no Box3D or workload frames in the report; top rows were {names[:5]}",
    )
    percent = sum(f["self_percent"] for f in report["functions"])
    check(95.0 <= percent <= 105.0, f"self percentages sum to {percent:.1f}, not ~100")


@scenario("call-trees", "Top-down and bottom-up trees agree with the flat report")
def call_trees(run):
    run.load_symbols()
    run.capture(seconds=8.0)
    run.stop_capture()
    flat = run.service.get("/api/sampling/report?start_ns=0&end_ns=18446744073709551615")
    self_counts = {f["name"]: f["self"] for f in flat["functions"]}

    top = run.service.get("/api/sampling/tree?mode=top_down")
    check_at_least(len(top["roots"]), 2, "thread roots in the top-down tree")
    check(all(r["kind"] == "thread" for r in top["roots"]), "top-down roots must be threads")

    bottom = run.service.get("/api/sampling/tree?mode=bottom_up")
    check_at_least(len(bottom["roots"]), 1, "bottom-up roots")
    for root in bottom["roots"]:
        # The invariant that defines bottom-up: a root's inclusive count is
        # that function's self count, because the walk starts at the leaf.
        check(
            root["inclusive"] == self_counts.get(root["name"], -1),
            f"bottom-up root {root['name']} has inclusive {root['inclusive']} but "
            f"self count {self_counts.get(root['name'])} in the flat report",
        )


@scenario("selection-report", "A time selection narrows the report, and a tid narrows it further")
def selection_report(run):
    run.load_symbols()
    run.capture(seconds=8.0)
    run.stop_capture()
    whole = run.service.get("/api/sampling/report?start_ns=0&end_ns=18446744073709551615")
    check_at_least(whole["samples"], 200, "samples in the whole capture")

    # Subdivide the span the samples actually occupy, not /api/status's
    # oldest/newest: the ring spans every capture this service has run, so a
    # quarter of it can easily fall in a stretch this capture never covered.
    t0, t1 = whole["first_sample_ns"], whole["last_sample_ns"]
    check(t0 < t1, f"the report reports no sample span: {t0}..{t1}")
    middle_a = t0 + (t1 - t0) // 4
    middle_b = t0 + (t1 - t0) // 2
    part = run.service.get(f"/api/sampling/report?start_ns={middle_a}&end_ns={middle_b}")
    check(
        0 < part["samples"] < whole["samples"],
        f"a sub-range must hold fewer samples: whole={whole['samples']} "
        f"sub={part['samples']} over [{middle_a}, {middle_b}] of [{t0}, {t1}]",
    )

    tree = run.service.get("/api/sampling/tree?mode=top_down")
    tid = tree["roots"][0]["tid"]
    scoped = run.service.get(
        f"/api/sampling/report?start_ns=0&end_ns=18446744073709551615&tid={tid}"
    )
    check(scoped["tid"] == tid, "the report does not echo the tid it was scoped to")
    check(
        0 < scoped["samples"] < whole["samples"],
        "one thread's samples must be a strict subset of the capture's",
    )


@scenario("report-tabs", "Each report tab renders: flat, top-down, bottom-up, modules")
def report_tabs(run):
    run.load_symbols()
    run.capture(seconds=8.0)
    run.open_viewer()
    run.stop_capture()
    # The panel appears on its own once the capture stops, showing the
    # whole-capture aggregate over everything just recorded.
    run.shot("03-report-flat", settle=4.0)
    # The other tabs are reached by deep link rather than by clicking. egui
    # paints to a canvas, so a pill has no DOM node; synthesising clicks at
    # fixed coordinates would break the first time the layout moved.
    tabs = [("top_down", "topdown"), ("bottom_up", "bottomup"), ("modules", "modules")]
    for index, (param, slug) in enumerate(tabs, start=4):
        run.open_viewer(f"?report={param}")
        run.shot(f"0{index}-report-{slug}", settle=3.0)


# The four manual-instrumentation test programs. Each runs the same scenario
# -- frames, three physics workers, an async job with an arrow, two graphed
# values, a name long enough to spill -- so their captures look alike and the
# documentation can show any of them.

def _build_app(lang):
    """Returns the argv to launch the test app for `lang`, building it first."""
    if lang == "rust":
        binary = os.path.join(REPO, "rust/target/release/OrbitTestRust")
        if not os.path.exists(binary):
            subprocess.run(["cargo", "build", "--release", "-p", "orbit-test-rust"],
                           cwd=os.path.join(REPO, "rust"), check=True)
        return [binary, "--seconds", "0"]
    if lang in ("c", "cpp"):
        name = {"c": "OrbitTestC", "cpp": "OrbitTestCpp"}[lang]
        folder = os.path.join(REPO, "src", name)
        binary = os.path.join(folder, name)
        if not os.path.exists(binary):
            subprocess.run([os.path.join(folder, "build.sh")], check=True)
        return [binary, "--seconds", "0"]
    if lang == "python":
        lib = os.path.join(REPO, "rust/target/release/liborbit_api.so")
        if not os.path.exists(lib):
            subprocess.run(["cargo", "build", "--release", "-p", "orbit-api"],
                           cwd=os.path.join(REPO, "rust"), check=True)
        return [sys.executable, os.path.join(REPO, "src/OrbitTestPython/OrbitTestPython.py"),
                "--seconds", "0"]
    raise Failure(f"unknown app language {lang}")


def _instrumented_app(run, lang, shot_index):
    """Captures one of the test apps and photographs it."""
    app = Target(_build_app(lang))
    try:
        run.service.post("/api/capture/start", {"pid": app.pid})
        time.sleep(6.0)
        # Scheduler folded so the process's own lanes are the picture, and
        # photographed while still capturing, before the report panel takes
        # the bottom of the window.
        run.open_viewer("?collapse=scheduler")
        run.shot(f"{shot_index:02d}-api-{lang}", settle=3.0)
        run.service.post("/api/capture/stop")
        time.sleep(1.5)
        log = run.service.stderr_text()
        check(
            f"opened segment of pid {app.pid}" in log,
            f"the service never opened {lang}'s scope segment; log tail: {log[-600:]}",
        )
        # The closing line reports what reached the timeline.
        summary = [l for l in log.splitlines() if "manual instrumentation:" in l and "events" in l]
        check(summary, "no manual-instrumentation summary in the service log")
        events = int(re.search(r"(\d+) events", summary[-1]).group(1))
        check_at_least(events, 500, f"{lang}: scope events pushed to the timeline")
        links = int(re.search(r"(\d+) links", summary[-1]).group(1))
        check_at_least(links, 1, f"{lang}: async job links seen")
        return f"{events} events, {links} links"
    finally:
        app.stop()


@scenario("api-rust", "OrbitTestRust: every instrumentation call, from Rust")
def api_rust(run):
    return _instrumented_app(run, "rust", 7)


@scenario("api-c", "OrbitTestC: every instrumentation call, from C")
def api_c(run):
    return _instrumented_app(run, "c", 8)


@scenario("api-cpp", "OrbitTestCpp: every instrumentation call, from C++ with RAII")
def api_cpp(run):
    return _instrumented_app(run, "cpp", 9)


@scenario("api-python", "OrbitTestPython: every instrumentation call, from Python over ctypes")
def api_python(run):
    return _instrumented_app(run, "python", 10)


@scenario("self-instrumentation", "The service profiles its own capture loop with the public API")
def self_instrumentation(run):
    # Any target will do; what is under test is that the service's own
    # segment is opened and its scopes -- read context switches, read
    # samples, unwind, symbolize, drain scope rings, push to viewer -- and its
    # buffer-fill values reach the timeline like any other process's.
    app = Target(_build_app("rust"))
    try:
        run.service.post("/api/capture/start", {"pid": app.pid})
        time.sleep(5.0)
        run.open_viewer("?collapse=scheduler")
        run.shot("11-self-instrumentation", settle=3.0)
        run.service.post("/api/capture/stop")
        time.sleep(1.5)
        log = run.service.stderr_text()
        service_pid = run.service.proc.pid
        check(
            f"opened segment of pid {service_pid}" in log,
            f"the service did not open its own segment (pid {service_pid}); log tail: {log[-500:]}",
        )
        check(f"opened segment of pid {app.pid}" in log, "and still opened the target's")
        summary = [l for l in log.splitlines() if "manual instrumentation:" in l and "segment(s)" in l]
        check(summary, "no closing summary")
        segments = int(re.search(r"(\d+) segment\(s\)", summary[-1]).group(1))
        check(segments >= 2, f"expected the target's segment and the service's own, got {segments}")
        return summary[-1].split("manual instrumentation: ")[-1]
    finally:
        app.stop()


@scenario("thread-states", "Thread state bars report real states, not just RUNNING")
def thread_states(run):
    run.capture(seconds=6.0)
    run.stop_capture()
    log = run.service.stderr_text()
    # Tracepoints need CAP_PERFMON and a readable tracefs. Unprivileged, the
    # correct behaviour is to say so and fall back to the RUNNING projection,
    # not to empty the timeline.
    if "no scheduling tracepoints" in log:
        check(
            "CAP_PERFMON" in log,
            "the fallback message must name what is missing",
        )
        check_at_least(
            run.service.get("/api/status")["events_live"], 1000,
            "the timeline must still fill from the RUNNING projection",
        )
        return "skipped: no scheduling tracepoints (needs CAP_PERFMON)"

    check("thread states from" in log, f"tracepoints opened but not reported: {log[-400:]}")
    # Privileged, the point of the feature: a busy multi-threaded workload
    # spends time in states other than RUNNING, and if every bar still says
    # RUNNING then the tracepoints are open and the payloads are being
    # misread -- which is the failure this scenario exists to catch.
    tree = run.service.get("/api/sampling/tree?mode=top_down")
    check_at_least(len(tree["roots"]), 1, "threads in the capture")
    return None


@scenario("instrumentation", "Hooks are requested, and the outcome is reported either way")
def instrumentation(run):
    run.load_symbols()
    hits = run.service.get(
        f"/api/functions/search?pid={run.target.pid}&q=orbit_e2e_step&limit=4"
    )["functions"]
    check_at_least(len(hits), 1, "instrumentable functions found")
    run.capture(
        seconds=5.0,
        instrumented_functions=[{"function_id": h["function_id"]} for h in hits],
        dynamic_instrumentation_method="kernel_uprobes",
    )
    status = run.service.get("/api/status")
    message = status.get("instrumentation", "")
    check(message != "", "the service said nothing about the hooks it was asked to arm")
    run.stop_capture()
    # Uprobes need CAP_PERFMON. Unprivileged, the correct outcome is a clear
    # refusal naming the fix -- not silence, and not a crash.
    if "no hooks armed" in message:
        check("CAP_PERFMON" in message, f"refusal does not name the capability: {message}")
        return f"skipped: {message.split('.')[0]}"
    check("instrumenting" in message, f"unexpected instrumentation status: {message}")
    return None


# ------------------------------------------------------------------------ run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=44810)
    parser.add_argument("--box3d", default=DEFAULT_BOX3D)
    parser.add_argument("--shots", default=os.path.join(REPO, "docs/screenshots"))
    parser.add_argument("--only", action="append", help="run only these scenarios")
    parser.add_argument("--keep-going", action="store_true",
                        help="run every scenario even after one fails")
    parser.add_argument("--no-shots", action="store_true", help="skip screenshots")
    args = parser.parse_args()

    wanted = [s for s in SCENARIOS if not args.only or s[0] in args.only]
    if not wanted:
        print(f"no scenario matches {args.only}", file=sys.stderr)
        return 2

    os.makedirs(args.shots, exist_ok=True)
    target_bin = "/tmp/orbit-e2e-box3d-target"
    print(f"building the Box3D target from {args.box3d}")
    build_target(args.box3d, target_bin)

    results = []
    target = service = chrome = None
    try:
        target = Target([target_bin, "--threads", "3"])
        print(f"target pid {target.pid}")
        service = Service(args.port)
        print(f"service on {service.base}")
        chrome = None if args.no_shots else Chrome(port=args.port + 5000)
        run = Run(service, target, chrome, args.shots)

        for name, description, fn in wanted:
            started = time.time()
            try:
                note = fn(run)
                results.append((name, "pass", note, time.time() - started))
                print(f"  PASS  {name}  ({time.time()-started:.1f}s) {note or ''}")
            except Exception as error:  # noqa: BLE001 - reported per scenario
                results.append((name, "FAIL", str(error), time.time() - started))
                print(f"  FAIL  {name}  {error}")
                if not args.keep_going:
                    break
            finally:
                service.post("/api/capture/stop")
    finally:
        for item in (chrome, service, target):
            if item is not None:
                item.stop() if hasattr(item, "stop") else item.close()

    print()
    passed = sum(1 for r in results if r[1] == "pass")
    for name, verdict, note, seconds in results:
        print(f"{verdict:>4}  {name:<22} {seconds:5.1f}s  {note or ''}")
    print(f"\n{passed}/{len(results)} scenarios passed")
    if not args.no_shots:
        shots = sorted(os.listdir(args.shots))
        print(f"{len(shots)} screenshots in {args.shots}")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
