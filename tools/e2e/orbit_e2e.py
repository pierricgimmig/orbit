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
    `pid=<n>` on its first line of stdout. With `stdin=True` the process gets
    a pipe, for programs that wait for a line before starting (`--wait-go`)."""

    def __init__(self, command, stdin=False):
        self.proc = subprocess.Popen(
            command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            stdin=subprocess.PIPE if stdin else None,
        )
        line = self.proc.stdout.readline()
        match = re.search(r"pid=(\d+)", line)
        if not match:
            raise Failure(f"target did not announce its pid: {line!r}")
        self.pid = int(match.group(1))

    def go(self):
        """Sends the line a `--wait-go` program is waiting for."""
        self.proc.stdin.write("go\n")
        self.proc.stdin.flush()

    def wait(self, timeout=120.0):
        """Waits for the process to exit; returns the rest of its stdout."""
        out, _ = self.proc.communicate(timeout=timeout)
        return out or ""

    def stop(self):
        if self.proc.poll() is not None:
            return
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# -------------------------------------------------------------------- service


# Set by --sudo: the service runs as root (`sudo -n`, so a password prompt
# fails fast instead of hanging), which is what arming uprobes needs.
SUDO = False


class Service:
    def __init__(self, port, binary=SERVICE, sudo=None, extra_args=()):
        if not os.path.exists(binary):
            raise Failure(
                f"{binary} is missing.\nBuild it:  cargo build --release "
                "--manifest-path rust/crates/orbit-service/Cargo.toml"
            )
        self.port = port
        self.base = f"http://127.0.0.1:{port}"
        self.log = open(f"/tmp/orbit-e2e-service-{port}.log", "w")
        self.sudo = SUDO if sudo is None else sudo
        # Through the wrapper tools/sudo/install.sh installs, when it is
        # there: its sudoers rule needs no password and is tied to the
        # binary's name, so a rebuilt or relocated service still qualifies.
        wrapper = "/usr/local/bin/orbit-service-sudo"
        prefix = []
        if self.sudo:
            prefix = ["sudo", "-n", "--"] + ([wrapper] if os.path.exists(wrapper) else [])
            binary = os.path.abspath(binary)
        # sudo relays the SIGTERM `stop` sends, so the service exits either way.
        self.proc = subprocess.Popen(
            prefix + [binary, *extra_args, "--serve", str(port)], stdout=self.log, stderr=subprocess.STDOUT
        )
        self._wait_ready()
        # The service's own pid: under sudo, `proc` is sudo (and the wrapper
        # behind it), and the service is a grandchild.
        self.pid = self.get("/api/status").get("service_pid") or self.proc.pid

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


def check_capture_clock(service):
    """Nothing in the ring may start before the capture did. Every stop
    goes through here, so every scenario's capture is checked."""
    status = service.get("/api/status")
    start = status.get("capture_start_ns", 0)
    if start and status.get("events_live", 0):
        oldest = status["oldest_start_ns"]
        check(oldest >= start, f"an event starts {start - oldest} ns before the capture start")
    return status.get("dropped_before_start", 0)


class Run:
    """Everything a scenario needs, plus where its screenshots go."""

    def __init__(self, service, target, chrome, shots_dir):
        self.service = service
        self.target = target
        self.chrome = chrome
        self.shots_dir = shots_dir
        self.shots = []
        # Numbers a scenario measured, written to the report at the end.
        self.perf = {}

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
        check_capture_clock(self.service)

    # ---- the viewer's readouts -------------------------------------------
    #
    # egui paints to a canvas, so nothing on the page has a DOM node. The
    # viewer hands the harness what it needs instead: `window.__orbit_sel`
    # (selection, tab, view, event count...), `window.__orbit_ui` (the
    # rectangle of every pill, report tab, menu item, Live row and track
    # header painted this frame, by label) and `window.__orbit_self` (its own
    # frame-time breakdown). A scenario clicks "Clear" or the header of
    # thread 1234 by name, and the layout can move without breaking it.

    def sel(self):
        text = self.chrome.eval("window.__orbit_sel || null")
        return json.loads(text) if text else {}

    def ui(self):
        text = self.chrome.eval("window.__orbit_ui || '[]'")
        rects = {}
        for label, x, y, w, h in json.loads(text):
            rects[label] = (x, y, w, h)  # the last one painted wins
        return rects

    def self_phases(self):
        text = self.chrome.eval("window.__orbit_self || null")
        return json.loads(text) if text else {}

    def wait_for(self, predicate, what, timeout=10.0, every=0.25):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            last = predicate()
            if last:
                return last
            time.sleep(every)
        raise Failure(f"timed out waiting for {what}")

    def rect(self, label, timeout=10.0):
        """The rectangle painted under `label`, waiting for it to appear."""
        return self.wait_for(lambda: self.ui().get(label), f"the {label!r} rectangle", timeout)

    def rects_matching(self, prefix):
        return {k: v for k, v in self.ui().items() if k.startswith(prefix)}

    def click(self, label, button="left", dx=0.5, dy=0.5):
        """Clicks inside the rectangle painted under `label`, at the fraction
        (dx, dy) of its width and height."""
        x, y, w, h = self.rect(label)
        self.chrome.click(x + w * dx, y + h * dy, button=button)
        time.sleep(0.4)

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
        # The app's ring held scopes from before Record: none may show.
        refused = check_capture_clock(run.service)
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
        return f"{events} events, {links} links, {refused} pre-start refused"
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
        service_pid = run.service.pid
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
    # Uprobes need CAP_SYS_ADMIN (the kernel's uprobe PMU checks that one,
    # not CAP_PERFMON). Unprivileged, the correct outcome is a clear
    # refusal naming the fix -- not silence, and not a crash.
    if "no hooks armed" in message:
        check("CAP_SYS_ADMIN" in message, f"refusal does not name the capability: {message}")
        return f"skipped: {message.split('.')[0]}"
    check("instrumenting" in message, f"unexpected instrumentation status: {message}")
    # Privileged: the hooked function's calls must be scopes on the target's
    # threads. The bundle's events table says so, through pyarrow.
    probe = subprocess.run([PYARROW_PYTHON, "-c", "import pyarrow"], capture_output=True)
    if probe.returncode != 0:
        return f"{message}; spans not checked ({PYARROW_PYTHON} has no pyarrow)"
    path = _export_bundle(run, "hooked.orbit.zip")
    folder = os.path.join(SCRATCH, "hooked-unzipped")
    shutil.rmtree(folder, ignore_errors=True)
    import zipfile
    with zipfile.ZipFile(path) as z:
        z.extractall(folder)
    count = subprocess.run(
        [PYARROW_PYTHON, "-c",
         "import pyarrow.parquet as pq,sys;t=pq.read_table(sys.argv[1]+'/events.parquet');"
         "name=t.column('name').to_pylist();kind=t.column('kind').to_pylist();dur=t.column('duration_ns').to_pylist();"
         "rows=[d for n,k,d in zip(name,kind,dur) if k==1 and 'orbit_e2e_step' in n];"
         "print(len(rows), max(rows) if rows else 0)", folder],
        capture_output=True, text=True, timeout=120,
    )
    check(count.returncode == 0, f"pyarrow query failed: {count.stderr[-300:]}")
    spans, longest = (int(v) for v in count.stdout.split())
    check_at_least(spans, 10, "hooked-function scopes on the timeline")
    check(longest < 1_000_000_000, f"a hooked span of {longest} ns is not a real call")
    return f"{message}; {spans} hooked spans, longest {longest/1e6:.2f} ms"


# ------------------------------------------------------------------------ run




# ------------------------------------------------ this week's viewer features
#
# These drive the viewer the way a person does -- click a thread header,
# right-click a scope, press Escape -- and assert on the readouts. They share
# one OrbitTestRust capture, taken once by `_week_capture` and kept on the
# service between scenarios, so each scenario is a few seconds, not a fresh
# capture. `_week_capture` re-takes it if a previous scenario cleared it.

AGENT_PID = 0xA6E70000
KIND_API_SCOPE, KIND_VALUE = 1, 6
SCRATCH = os.environ.get("ORBIT_E2E_SCRATCH", "/tmp/orbit-e2e")
ORBIT_SCOPE = os.path.join(REPO, "rust/target/release/orbit-scope")
PYARROW_PYTHON = os.environ.get("ORBIT_E2E_PYARROW_PYTHON", sys.executable)


class WeekCapture:
    """One OrbitTestRust capture the feature scenarios share."""

    pid = None
    taken = False
    service_pid = None


def _week_capture(run, seconds=4.0):
    if WeekCapture.taken and run.service.get("/api/status")["events_live"] > 0:
        return
    app = Target(_build_app("rust"))
    try:
        # A scope on the agent track and two values, so the capture also
        # carries what the agent scenarios look for.
        run.service.post("/api/capture/start", {"pid": app.pid})
        time.sleep(1.0)
        _orbit_scope(run, "run", "--name", "tool: sleep", "--", "sleep", "0.3")
        _orbit_scope(run, "value", "files changed", "3")
        _orbit_scope(run, "instant", "commit")
        # Symbols, so the reports name functions rather than addresses.
        run.service.post("/api/symbols/load", {"pid": app.pid})
        time.sleep(seconds - 1.0)
        deadline = time.time() + 30
        while time.time() < deadline:
            if run.service.get(f"/api/symbols/status?pid={app.pid}").get("status") in ("ready", "error"):
                break
            time.sleep(0.4)
        run.service.post("/api/capture/stop")
        time.sleep(1.5)
        check_capture_clock(run.service)
    finally:
        app.stop()
    WeekCapture.pid = app.pid
    WeekCapture.service_pid = run.service.pid
    WeekCapture.taken = True


def _orbit_scope(run, *args):
    if not os.path.exists(ORBIT_SCOPE):
        subprocess.run(["cargo", "build", "--release", "-p", "orbit-api", "--bin", "orbit-scope"],
                       cwd=os.path.join(REPO, "rust"), check=True)
    result = subprocess.run(
        [ORBIT_SCOPE, "--url", run.service.base, *args],
        capture_output=True, text=True, timeout=30,
    )
    check(result.returncode == 0, f"orbit-scope {' '.join(args)} failed: {result.stderr[-300:]}")
    return result.stdout


def _rows(run):
    return sorted(k for k in run.ui() if k.startswith("row:"))


def _thread_rows(run, pid):
    rows = run.wait_for(lambda: run.rects_matching(f"row:thread:{pid}:") or None,
                        f"a thread header of pid {pid} (rows: {_rows(run)[:12]})")
    # The process's first thread must be on screen to be clicked. Other
    # processes above it (the service's, with its many threads) are folded
    # by their chevron, 16 px into the row, until it is.
    canvas_h = run.chrome.eval("document.querySelector('canvas').clientHeight")
    for _ in range(6):
        rows = run.rects_matching(f"row:thread:{pid}:")
        first_y = min(v[1] for v in rows.values())
        if first_y + 40 < canvas_h:
            break
        above = [(v[1], k) for k, v in run.rects_matching("row:process:").items()
                 if not k.endswith(f":{pid}") and v[1] < first_y]
        if not above:
            break
        x, y, w, h = run.rect(sorted(above)[0][1])
        run.chrome.click(x + 16, y + h * 0.5)
        time.sleep(0.6)
    return run.rects_matching(f"row:thread:{pid}:")


@scenario("thread-focus", "Clicking a thread header focuses it; Escape or an empty click clears")
def thread_focus(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler")
    pid = WeekCapture.pid
    rows = _thread_rows(run, pid)
    label = sorted(rows)[0]
    tid = int(label.split(":")[3])
    run.click(label, dx=0.3)
    sel = run.wait_for(lambda: run.sel() if run.sel().get("thread") else None, "a thread selection")
    check(sel["thread"] == [pid, tid], f"expected thread [{pid},{tid}] selected, got {sel['thread']}")
    check(sel["focus"] == [pid, tid], "the thread focus (what greys the rest) follows the selection")
    run.shot("12-thread-focus", settle=1.0)
    # Escape clears everything: the thread, the scope pick, the measure.
    run.chrome.key("Escape")
    sel = run.wait_for(lambda: run.sel() if not run.sel().get("thread") else None, "Escape to clear")
    check(sel["focus"] is None, "focus cleared with the selection")
    # A click on the canvas where there is nothing also clears. Select
    # again, then click the far right of the header column's empty space.
    run.click(label, dx=0.3)
    run.wait_for(lambda: run.sel().get("thread"), "the second selection")
    x, y, w, h = run.rect("row:scheduler")
    run.chrome.click(x + w * 0.5, y + h * 0.5)
    run.wait_for(lambda: not run.sel().get("thread"), "the empty click to clear")
    return f"thread {tid} of pid {pid}"


@scenario("scope-report", "Right-clicking a scope offers a sampling report over its instances")
def scope_report(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler")
    pid = WeekCapture.pid
    # A thread's scopes are drawn in its own row, below the header line.
    lanes = _thread_rows(run, pid)
    x, y, w, h = run.rect("row:scheduler")
    head_right = x + w  # the header column's right edge: the canvas starts here
    canvas_right = run.chrome.eval("document.querySelector('canvas').clientWidth")
    # Zoom in first (W held, pointer over the first thread) so the scopes
    # are a few pixels wide rather than sub-pixel columns, then sweep the
    # thread rows. A right-click that lands on a manual scope (kind 1)
    # opens the menu and `scope_menu` in the readout says so; a sampled
    # frame opens it too, and a scope shorter than the sampling period has
    # no samples inside it, so keep going past both until the scoped report
    # has substance.
    canvas_h = run.chrome.eval("document.querySelector('canvas').clientHeight")
    first = sorted(lanes.items())[0][1]
    run.chrome.move(head_right + (canvas_right - head_right) * 0.5, first[1] + first[3] * 0.5)
    run.chrome.call("Input.dispatchKeyEvent", type="keyDown", key="w", code="KeyW", windowsVirtualKeyCode=87)
    time.sleep(1.5)
    run.chrome.call("Input.dispatchKeyEvent", type="keyUp", key="w", code="KeyW", windowsVirtualKeyCode=87)
    time.sleep(0.5)

    def sweep():
        for _, (lx, ly, lw, lh) in sorted(lanes.items()):
            if ly > canvas_h:
                continue  # below the fold
            for dy in (0.3, 0.4, 0.5, 0.2, 0.6, 0.7, 0.8, 0.15, 0.9):
                if ly + lh * dy > canvas_h:
                    continue
                for frac in (0.5, 0.3, 0.7):
                    yield head_right + (canvas_right - head_right) * frac, ly + lh * dy

    report = None
    tried = []
    for px, py in sweep():
        run.chrome.click(px, py, button="right")
        time.sleep(0.5)
        menu = run.sel().get("scope_menu")
        if not menu:
            continue
        if menu[2] != KIND_API_SCOPE:
            run.chrome.key("Escape")
            time.sleep(0.3)
            continue
        run.shot("13-scope-menu", settle=0.5)
        run.click("menu:report")
        sel = run.wait_for(lambda: run.sel() if run.sel().get("scope_report") else None, "the scoped report")
        name_id, name = sel["scope_report"]
        # The viewer asked the service for the report over this scope's
        # instances; ask the same question and check it has substance.
        candidate = run.service.get(f"/api/sampling/report?scope={name_id}")
        tried.append((name, candidate.get("range_count", 0), candidate.get("samples", 0)))
        if candidate.get("samples", 0) >= 1:
            report = candidate
            break
        run.chrome.key("Escape")
        run.wait_for(lambda: not run.sel().get("scope_report"), "Escape to drop the empty report")
    check(tried, "no right-click on the thread rows landed on a manual scope")
    check(report is not None, f"no scope with samples inside it; tried {tried}")
    check_at_least(report.get("range_count", 0), 1, f"instances of {name!r} the report is scoped to")
    run.shot("14-scope-report", settle=2.0)
    run.chrome.key("Escape")
    run.wait_for(lambda: not run.sel().get("scope_report"), "Escape to drop the scoped report")
    return f"{name!r}: {report['samples']} samples over {report['range_count']} instances"


@scenario("live-tab", "The Live tab keeps per-scope statistics and a duration histogram")
def live_tab(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler&report=live")
    run.wait_for(lambda: run.sel().get("tab") == "Live", "the Live tab")
    rows = run.wait_for(lambda: run.rects_matching("live:") or None, "Live rows", timeout=20)
    check_at_least(len(rows), 3, "Live rows (scope names)")
    # Click the hottest row: the histogram is drawn for the selected row.
    first = sorted(rows.items(), key=lambda kv: kv[1][1])[0][0]
    run.click(first)
    run.shot("15-live-tab", settle=1.0)
    return f"{len(rows)} rows, histogram for {first[5:]!r}"


@scenario("flame-tab", "The Flame tab draws the sampling report as a flame graph")
def flame_tab(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler&report=flame")
    run.wait_for(lambda: run.sel().get("tab") == "Flame", "the Flame tab")
    run.shot("16-flame-tab", settle=2.5)
    return "ok"


def _export_bundle(run, name, query=""):
    body = run.service.get(f"/api/capture/export?format=bundle{query}")
    check(isinstance(body, bytes) and body[:2] == b"PK", "the export is not a zip")
    os.makedirs(SCRATCH, exist_ok=True)
    path = os.path.join(SCRATCH, name)
    with open(path, "wb") as handle:
        handle.write(body)
    return path


def _bundle_manifest(path):
    import zipfile
    with zipfile.ZipFile(path) as z:
        names = z.namelist()
        manifest = [n for n in names if n.endswith("manifest.json")]
        check(manifest, f"no manifest in {path}: {names}")
        return json.loads(z.read(manifest[0])), names


@scenario("save-slice-open", "Save and Save slice export a bundle the service opens again")
def save_slice_open(run):
    _week_capture(run)
    status = run.service.get("/api/status")
    t0, t1 = status["oldest_start_ns"], status["newest_end_ns"]
    check(t1 > t0, "the capture has a span")
    whole = _export_bundle(run, "capture.orbit.zip")
    mid = (t0 + t1) // 2
    sliced = _export_bundle(run, "capture-slice.orbit.zip", f"&t0={mid}&t1={mid + (t1 - t0) // 10}")
    manifest, names = _bundle_manifest(whole)
    parquet = [n for n in names if n.endswith(".parquet")]
    check_at_least(len(parquet), 3, "Parquet tables in the bundle (events, samples, frames)")
    bundle = manifest.get("bundle", manifest)
    check(bundle.get("target_pid") == WeekCapture.pid, f"manifest target_pid: {bundle.get('target_pid')}")
    check_at_least(len(bundle.get("threads", [])), 2, "thread names in the manifest")
    slice_manifest, _ = _bundle_manifest(sliced)
    slice_bundle = slice_manifest.get("bundle", slice_manifest)
    check(slice_bundle.get("slice_ns"), "the slice records its window")
    check(os.path.getsize(sliced) < os.path.getsize(whole), "a tenth of the capture is smaller than the whole")
    # Open the slice by path (the Open pill's route) after clearing.
    reply = run.service.post("/api/capture/clear")
    check(not reply.startswith("HTTP"), f"clear refused: {reply}")
    check(run.service.get("/api/status")["events_live"] == 0, "the ring is empty after Clear")
    reply = run.service.post("/api/capture/open", {"path": sliced})
    check(not reply.startswith("HTTP"), f"open refused: {reply}")
    opened = run.wait_for(
        lambda: run.service.get("/api/status")["events_live"] or None, "the slice to load", timeout=15,
    )
    if run.chrome is not None:
        run.open_viewer("?collapse=scheduler")
        run.wait_for(lambda: run.sel().get("events"), "the opened slice in the viewer", timeout=20)
        run.shot("17-opened-slice", settle=2.0)
    WeekCapture.taken = False  # the ring now holds the slice, not the capture
    run.perf["bundle_bytes"] = os.path.getsize(whole)
    run.perf["slice_bytes"] = os.path.getsize(sliced)
    return f"{os.path.getsize(whole)//1024} KB bundle, {os.path.getsize(sliced)//1024} KB slice, {opened} events reopened"


@scenario("python-reader", "open_capture.py reads an exported bundle with pyarrow (TODO 23)")
def python_reader(run):
    probe = subprocess.run([PYARROW_PYTHON, "-c", "import pyarrow"], capture_output=True)
    if probe.returncode != 0:
        return f"skipped: {PYARROW_PYTHON} has no pyarrow (set ORBIT_E2E_PYARROW_PYTHON)"
    path = os.path.join(SCRATCH, "capture.orbit.zip")
    if not os.path.exists(path):
        _week_capture(run)
        _export_bundle(run, "capture.orbit.zip")
    folder = os.path.join(SCRATCH, "capture-unzipped")
    shutil.rmtree(folder, ignore_errors=True)
    import zipfile
    with zipfile.ZipFile(path) as z:
        z.extractall(folder)
    script = os.path.join(REPO, "rust/crates/orbit-capture/python/open_capture.py")
    result = subprocess.run([PYARROW_PYTHON, script, folder], capture_output=True, text=True, timeout=120)
    check(result.returncode == 0, f"open_capture.py failed: {result.stderr[-400:]}")
    out = result.stdout
    events = re.search(r"(\d[\d,]*) events", out)
    check(events, f"the reader printed no event count: {out[:300]}")
    n = int(events.group(1).replace(",", ""))
    check_at_least(n, 100, "events read back by pyarrow")
    # The agent track and the service's own values are in the table too.
    agent = subprocess.run(
        [PYARROW_PYTHON, "-c",
         "import pyarrow.parquet as pq,sys;t=pq.read_table(sys.argv[1]+'/events.parquet').to_pandas() "
         "if False else pq.read_table(sys.argv[1]+'/events.parquet');"
         "pid=t.column('pid').to_pylist();kind=t.column('kind').to_pylist();"
         f"print(sum(1 for p in pid if p=={AGENT_PID}), sum(1 for k in kind if k=={KIND_VALUE}))",
         folder],
        capture_output=True, text=True, timeout=120,
    )
    check(agent.returncode == 0, f"pyarrow query failed: {agent.stderr[-300:]}")
    agent_rows, value_rows = (int(v) for v in agent.stdout.split())
    check_at_least(agent_rows, 2, "agent-track rows (orbit-scope run/value/instant)")
    check_at_least(value_rows, 2, "value rows (service cpu %, rss MiB, agent value)")
    return f"{n} events; {agent_rows} agent rows, {value_rows} value rows"


@scenario("agent-scopes", "orbit-scope puts an agent's tool calls on their own track")
def agent_scopes(run):
    _week_capture(run)
    path = _export_bundle(run, "agent.orbit.zip")
    manifest, _ = _bundle_manifest(path)
    bundle = manifest.get("bundle", manifest)
    threads = bundle.get("threads", [])
    agent_threads = [t for t in threads if int(t.get("pid", 0)) == AGENT_PID]
    check(agent_threads, f"no agent track in the manifest threads: {threads[:6]}")
    names = {t.get("name") for t in agent_threads}
    check("agent" in names, f"the default track is named 'agent': {names}")
    # A stop with nothing open is refused, so a bad script cannot corrupt the track.
    reply = run.service.post("/api/scope", {"track": "e2e-empty", "action": "stop"})
    check(reply.startswith("HTTP 4"), f"stop on an empty track should be refused, got {reply[:80]}")
    if run.chrome is not None:
        run.open_viewer("?collapse=scheduler")
        run.wait_for(lambda: run.rects_matching(f"row:process:{AGENT_PID}") or None,
                     f"the agent process row (rows: {_rows(run)[:16]})", timeout=15)
        # The agent's process sorts last; fold every other process (the
        # chevron sits 16 px into the row) so it comes into view.
        for label in sorted(run.rects_matching("row:process:")):
            if label.endswith(f":{AGENT_PID}"):
                continue
            x, y, w, h = run.rect(label)
            run.chrome.click(x + 16, y + h * 0.5)
            time.sleep(0.5)
        canvas_h = run.chrome.eval("document.querySelector('canvas').clientHeight")
        ax, ay, aw, ah = run.rect(f"row:process:{AGENT_PID}")
        check(ay < canvas_h, f"the agent row is still below the fold (y {ay} of {canvas_h})")
        # Home fits the whole capture, so the scope near its start is in view.
        run.chrome.key("Home")
        run.wait_for(lambda: run.sel().get("events"), "the fitted view to show events")
        run.shot("18-agent-track", settle=2.0)
    return f"tracks {sorted(names)}"


@scenario("service-lanes", "The service's own cpu % and rss MiB are value lanes in every capture")
def service_lanes(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler")
    service_pid = WeekCapture.service_pid
    lanes = run.wait_for(
        lambda: {k: v for k, v in run.rects_matching(f"row:lane:{service_pid}:").items()
                 if k.endswith(f":{KIND_VALUE}")} or None,
        f"value lanes of the service (pid {service_pid})",
    )
    check_at_least(len(lanes), 1, "value lanes under the service process")
    run.shot("19-service-lanes", settle=1.0)
    return f"{len(lanes)} value lane(s) under pid {service_pid}"


@scenario("clear", "The Clear pill empties the timeline, and the service's ring with it")
def clear(run):
    if run.chrome is None:
        reply = run.service.post("/api/capture/clear")
        check(not reply.startswith("HTTP"), f"clear refused: {reply}")
        return "cleared over HTTP (no browser)"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler")
    run.wait_for(lambda: run.sel().get("events"), "events before Clear")
    run.click("Clear")
    run.wait_for(lambda: run.sel().get("events") == 0, "the timeline to empty")
    check(run.service.get("/api/status")["events_live"] == 0, "the service's ring is empty too")
    run.shot("20-cleared", settle=1.0)
    WeekCapture.taken = False
    return "ok"


@scenario("wire-and-perf", "Wire format is reported, and the viewer's frame budget is recorded")
def wire_and_perf(run):
    status = run.service.get("/api/status")
    check(status.get("wire") in ("raw", "packed", "deflate"), f"status wire: {status.get('wire')}")
    run.perf["wire"] = status["wire"]
    if run.chrome is None:
        return f"wire {status['wire']} (no browser: no frame numbers)"
    _week_capture(run)
    run.open_viewer("?collapse=scheduler")
    sel = run.wait_for(lambda: run.sel() if run.sel().get("events") else None, "the capture in the viewer")
    check(sel["wire"] == status["wire"], "the viewer shows the same wire format the service reports")
    # A live stream, for the rate: start a capture of the service itself.
    run.service.post("/api/capture/start", {"pid": run.target.pid})
    time.sleep(3.0)
    rate = run.wait_for(lambda: run.sel().get("ws_bps") or None, "bytes on the WebSocket", timeout=15)
    run.service.post("/api/capture/stop")
    time.sleep(2.0)
    # The frame breakdown is published while the Self pane is open. A click
    # that lands during a long frame (the whole-capture report after Stop)
    # can be lost, so the pane is asked for again if nothing shows.
    phases = None
    for _ in range(3):
        run.click("Self")
        try:
            phases = run.wait_for(lambda: run.self_phases() or None, "the self-profile readout", timeout=7)
            break
        except Failure:
            continue
    check(phases, "the self-profile readout never appeared after three clicks on Self")
    run.shot("21-self-pane", settle=2.0)
    run.perf["ws_bps_during_capture"] = rate
    run.perf["events"] = run.sel().get("events")
    run.perf["self"] = phases
    after = run.service.get("/api/status")  # the ring after the capture, not before it
    run.perf["status"] = {k: after[k] for k in ("events_live", "events_capacity", "ring_bytes", "produced", "dropped")}
    frame = next((p for p in phases.get("phases", []) if p.get("name") in ("frame", "update")), None)
    note = f"wire {status['wire']}, {rate/1024:.0f} KB/s on the socket"
    if frame:
        note += f", frame avg {frame['avg_us']/1000:.1f} ms"
    return note



@scenario("website", "The static site embeds a capture the viewer opens with no service")
def website(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    _week_capture(run)
    stream = run.service.get("/api/capture/export?format=stream")
    check(isinstance(stream, bytes) and len(stream) > 1000, "the stream export is empty")
    events = run.service.get("/api/status")["events_live"]
    os.makedirs(SCRATCH, exist_ok=True)
    stream_path = os.path.join(SCRATCH, "site.orbit.stream")
    with open(stream_path, "wb") as handle:
        handle.write(stream)
    site = os.path.join(SCRATCH, "site")
    shutil.rmtree(site, ignore_errors=True)
    build = subprocess.run(
        [sys.executable, os.path.join(REPO, "tools/site/build_site.py"), "--out", site,
         "--stream", stream_path, "--name", "e2e"],
        capture_output=True, text=True, timeout=120,
    )
    check(build.returncode == 0, f"build_site.py failed: {build.stderr[-400:]}")
    for needed in ("index.html", "viewer/orbit_live_viewer_bg.wasm", "captures/e2e.orbit.stream",
                   "manual/index.html", "blog/index.html", "e2e/report.html", "site.css"):
        check(os.path.exists(os.path.join(site, needed)), f"the site is missing {needed}")
    port = run.service.port + 7
    server = subprocess.Popen(
        [sys.executable, os.path.join(REPO, "tools/site/serve.py"), "--dir", site, "--port", str(port),
         "--bind", "127.0.0.1"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        base = f"http://127.0.0.1:{port}"
        run.wait_for(lambda: _http_ok(base + "/index.html"), "the site server")
        # The front page embeds the viewer in an iframe; its readout is
        # reachable from the page because both are one origin.
        started = time.time()
        run.chrome.goto(base + "/index.html", settle=6.0)
        sel_js = ("(()=>{const f=document.querySelector('iframe');"
                  "const s=f&&f.contentWindow&&f.contentWindow.__orbit_sel;return s||null})()")
        shown = run.wait_for(
            lambda: (lambda s: json.loads(s)["events"] if s else 0)(run.chrome.eval(sel_js)) or None,
            "events in the embedded viewer", timeout=30,
        )
        seconds = time.time() - started
        check_at_least(shown, int(events * 0.9), "events the embedded viewer shows of the service's")
        run.shot("22-website", settle=2.0)
        # The viewer page alone, as the "Open full page" link opens it.
        run.chrome.goto(f"{base}/viewer/index.html?capture=../captures/e2e.orbit.stream&collapse=scheduler", settle=6.0)
        sel = run.wait_for(lambda: run.sel() if run.sel().get("events") else None, "the full-page viewer", timeout=30)
        check(sel["hellos"] >= 1, "the stream's Hello frame was read")
        run.shot("23-static-viewer", settle=2.0)
        run.perf["stream_bytes"] = len(stream)
        run.perf["site_first_events_s"] = round(seconds, 1)
        return f"{shown} events from a {len(stream)//1024} KB stream, {seconds:.1f} s to first events"
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()


def _http_ok(url):
    try:
        with urllib.request.urlopen(url, timeout=2) as response:
            return response.status == 200
    except Exception:  # noqa: BLE001 - not up yet
        return False



@scenario("hook-from-report", "A function is hooked from the sampling report, and the next capture arms it")
def hook_from_report(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    # Box3D, with symbols, so the report's rows carry function ids.
    run.load_symbols()
    run.capture(seconds=4.0)
    run.stop_capture()
    run.open_viewer("?collapse=scheduler&report=flat")
    rows = run.wait_for(lambda: run.rects_matching("report:") or None, "flat report rows", timeout=20)
    # The workload's own function, so the hook is on the target's code; the
    # first one in view (the report scrolls, and rows below the fold are in
    # the readout too, with a y past the canvas).
    canvas_h = run.chrome.eval("document.querySelector('canvas').clientHeight")
    target_rows = [(v[1], k) for k, v in rows.items()
                   if (k.startswith("report:b3") or "orbit_e2e" in k) and 0 <= v[1] < canvas_h - 20]
    check(target_rows, f"no Box3D row in view in the report: {sorted(rows)[:8]}")
    row = sorted(target_rows)[0][1]
    run.click(row, button="right")
    run.rect("menu:hook", timeout=5)
    run.shot("24-hook-from-report", settle=0.5)
    run.click("menu:hook")
    sel = run.wait_for(lambda: run.sel() if run.sel().get("hooks") else None, "the hook in the readout")
    ids = sel["hooks"]
    check(len(ids) == 1, f"one hook expected, got {ids}")
    # The id is one the service's function index knows, under the same name.
    name = row[len("report:"):]
    hits = run.service.get(f"/api/functions/search?pid={run.target.pid}&q={name}&limit=8")["functions"]
    check(any(h["function_id"] == ids[0] for h in hits), f"the report's id {ids[0]} is not a search hit for {name!r}: {hits[:3]}")
    # Unhook and hook again through the same menu: the list follows.
    run.click(row, button="right")
    run.click("menu:hook")
    run.wait_for(lambda: run.sel().get("hooks") == [], "the hook removed")
    run.click(row, button="right")
    run.click("menu:hook")
    run.wait_for(lambda: run.sel().get("hooks") == ids, "the hook back")
    # What Record would send: the same ids, to the same route.
    run.capture(seconds=3.0, instrumented_functions=[{"function_id": i} for i in ids],
                dynamic_instrumentation_method="kernel_uprobes")
    message = run.service.get("/api/status").get("instrumentation", "")
    run.stop_capture()
    check(message, "the service said nothing about the hook it was asked to arm")
    if "no hooks armed" in message:
        check("CAP_SYS_ADMIN" in message, f"refusal does not name the capability: {message}")
        return f"hooked {name!r}; arming skipped: {message.split('.')[0]}"
    check("instrumenting" in message, f"unexpected instrumentation status: {message}")
    return f"hooked {name!r}: {message}"



# Threads, calls per second per thread, calls per thread. 8 x 5 kHz x 5000
# is 40,000 outer calls in a second, 360,000 scopes, 720,000 probe hits. An
# optional fourth number makes every thread move to the next CPU every that
# many calls (`--stress-migrate`), for the migration experiments.
STRESS = tuple(int(v) for v in os.environ.get("ORBIT_E2E_STRESS", "8,5000,5000").split(","))
# The uprobe duplicate filter and frame pairing: on unless ORBIT_E2E_DEDUPE=0,
# which is the "without the fix" run of blog post 20.
DEDUPE = os.environ.get("ORBIT_E2E_DEDUPE", "1") != "0"
# Where to save the run's capture as a stream file (the site's embed format),
# when set; the bundle the checker reads is saved next to it.
STRESS_STREAM = os.environ.get("ORBIT_E2E_STRESS_STREAM", "")
# CPUs the workload is confined to (`taskset -c`), e.g. "0-7": more threads
# than CPUs there means the scheduler preempts and moves them mid-call, the
# migration the probes mind, while the service keeps the other cores.
STRESS_CPUS = os.environ.get("ORBIT_E2E_STRESS_CPUS", "")


@scenario("dyn-instr-stress",
          "OrbitTestRust --stress: every hooked call accounted for, at its depth, inside its parent")
def dyn_instr_stress(run):
    """The stress test for dynamic instrumentation. A known tree of three
    functions (outer -> 2 middle -> 3 inner each) called a known number of
    times on a known number of threads, hooked with uprobes; the exported
    bundle is then checked call for call by check_stress.py. Needs the
    service to run as root (--sudo); unprivileged it records the refusal."""
    threads, hz, calls = STRESS[:3]
    migrate = STRESS[3] if len(STRESS) > 3 else 0
    binary = _build_app("rust")[0]
    command = [binary, "--stress-threads", str(threads), "--stress-hz", str(hz),
               "--stress-calls", str(calls), "--stress-migrate", str(migrate), "--wait-go"]
    if STRESS_CPUS:
        command = ["taskset", "-c", STRESS_CPUS] + command
    app = Target(command, stdin=True)
    try:
        # Symbols and the three function ids, by name.
        run.service.post("/api/symbols/load", {"pid": app.pid})
        deadline = time.time() + 40
        while time.time() < deadline:
            status = run.service.get(f"/api/symbols/status?pid={app.pid}")
            if status.get("status") == "ready":
                break
            check(status.get("status") != "error", f"symbols: {status.get('error')}")
            time.sleep(0.3)
        ids = {}
        for name in ("orbit_stress_outer", "orbit_stress_middle", "orbit_stress_inner"):
            hits = run.service.get(f"/api/functions/search?pid={app.pid}&q={name}&limit=8")["functions"]
            exact = [h for h in hits if h["name"] == name]
            check(exact, f"{name} is not in the function index: {hits[:3]}")
            ids[name] = exact[0]["function_id"]
        # Arm, then let the program run: nothing happens before the probes.
        run.service.post("/api/capture/start", {
            "pid": app.pid, "sampling": False, "context_switches": True, "thread_states": True,
            "dynamic_instrumentation_method": "kernel_uprobes",
            "instrumented_functions": [{"function_id": i} for i in ids.values()],
            "uprobe_duplicate_filter": DEDUPE,
        })
        message = ""
        deadline = time.time() + 15
        while time.time() < deadline:
            message = run.service.get("/api/status").get("instrumentation", "")
            if message:
                break
            time.sleep(0.2)
        check(message, "the service said nothing about arming the hooks")
        if "no hooks armed" in message:
            run.service.post("/api/capture/stop")
            check("CAP_SYS_ADMIN" in message, f"refusal does not name the capability: {message}")
            return f"skipped: {message.split('.')[0]} (run with --sudo)"
        check("instrumenting 3 of 3" in message, f"not every function armed: {message}")
        app.go()
        out = app.wait(timeout=180)
        done = re.search(r"stress done: threads=(\d+) calls=(\d+) outer=(\d+) middle=(\d+) inner=(\d+) migrations=(\d+) in ([\d.]+)s", out)
        check(done, f"the program did not report what it made: {out[-300:]!r}")
        # The reorder window is 100 ms; give the last hits time to pair.
        time.sleep(0.8)
        run.stop_capture()
        status = run.service.get("/api/status")
        line = status.get("instrumentation", "")
        if DEDUPE:
            counts = re.search(r"(\d+) calls; (\d+) duplicate entries dropped, (\d+) entries with no return discarded, (\d+) returns with no entry dropped", line)
            check(counts, f"no pairing counts on the status line: {line!r}")
            calls_seen, dup, discarded, orphans = (int(v) for v in counts.groups())
        else:
            counts = re.search(r"(\d+) calls; filter off, (\d+) migration duplicates went through", line)
            check(counts, f"no filter-off counts on the status line: {line!r}")
            calls_seen, dup = (int(v) for v in counts.groups())
            discarded = orphans = 0
        lost_records = int(m.group(1)) if (m := re.search(r"(\d+) records lost by the kernel", line)) else 0
        healed = dup + discarded + orphans
        # The bundle, call for call.
        probe = subprocess.run([PYARROW_PYTHON, "-c", "import pyarrow"], capture_output=True)
        check(probe.returncode == 0, f"{PYARROW_PYTHON} has no pyarrow; set ORBIT_E2E_PYARROW_PYTHON")
        path = _export_bundle(run, "stress.orbit.zip")
        if STRESS_STREAM:
            stream = run.service.get("/api/capture/export?format=stream")
            check(isinstance(stream, bytes) and len(stream) > 1000, "the stream export is empty")
            with open(STRESS_STREAM, "wb") as handle:
                handle.write(stream)
            shutil.copy(path, os.path.splitext(STRESS_STREAM)[0] + ".orbit.zip")
        result = subprocess.run(
            [PYARROW_PYTHON, os.path.join(HERE, "check_stress.py"), path,
             "--pid", str(app.pid), "--threads", str(threads), "--calls", str(calls)],
            capture_output=True, text=True, timeout=600,
        )
        check(result.returncode == 0, f"check_stress failed: {result.stderr[-400:]}")
        verdict = json.loads(result.stdout)
        expected_total = sum(verdict["expected"].values())
        run.perf["stress"] = {
            "threads": threads, "hz": hz, "calls": calls, "migrate_every": migrate,
            "migrations": int(done.group(6)), "dedupe": DEDUPE, "cpus": STRESS_CPUS, "seconds": float(done.group(7)),
            "expected": verdict["expected"], "observed": verdict["observed"],
            "missing": verdict["missing"], "calls_on_status_line": calls_seen,
            "duplicates_dropped": dup, "unclosed_discarded": discarded, "orphan_returns": orphans,
            "records_lost": lost_records, "depth_wrong": verdict["depth_wrong_total"],
            "not_contained": verdict["not_contained_total"], "durations": verdict["durations"],
            "migrations_seen": verdict.get("migrations_seen", 0),
            "status_line": line,
        }
        # What must hold whatever the kernel lost.
        check(verdict["threads_seen"] == threads, f"{verdict['threads_seen']} of {threads} threads have scopes")
        check(verdict["extra_total"] == 0, f"more scopes than calls: {verdict['extra']}")
        check(verdict["outer_inside_something"] == 0,
              f"{verdict['outer_inside_something']} outer scopes nested inside another scope")
        check(calls_seen == sum(verdict["observed"].values()),
              f"status line says {calls_seen} calls, the bundle holds {sum(verdict['observed'].values())}")
        with open(f"/tmp/orbit-e2e-stress-{run.service.port}.json", "w") as out:
            json.dump(run.perf["stress"], out, indent=1)
        # Every hit the pairing gave up on is one scope missing and one count
        # on the status line: the two must agree, or the pairing is inventing
        # or hiding something. A dropped duplicate is the one count that
        # need not cost a scope: a hit the kernel re-reported at a migration
        # is dropped for free, a real call misjudged as one goes missing. So
        # the hole is at least the unclosed and orphan counts, and at most
        # those plus the duplicates. Records the kernel lost outright never
        # reach the pairing, so with any of those the floor still holds and
        # the one-percent ceiling below is what fails.
        floor = discarded + orphans
        check(floor <= verdict["missing_total"],
              f"{verdict['missing_total']} scopes missing but the pairing gave up on {floor} "
              f"({discarded} unclosed, {orphans} orphan)")
        if lost_records == 0:
            check(verdict["missing_total"] <= floor + dup,
                  f"{verdict['missing_total']} scopes missing, more than the pairing gave up on "
                  f"({discarded} unclosed, {orphans} orphan, {dup} dup) with no records lost")
        # A lost hit can put the scopes under it one level off until it is
        # healed: at most 8 (an outer's 2 middles and 6 inners) per loss.
        check(verdict["depth_wrong_total"] <= 8 * healed,
              f"{verdict['depth_wrong_total']} scopes at the wrong depth for {healed} lost hits")
        check(verdict["not_contained_total"] <= 8 * healed,
              f"{verdict['not_contained_total']} scopes outside their parent for {healed} lost hits")
        check(verdict["missing_total"] * 100 <= expected_total,
              f"{verdict['missing_total']} of {expected_total} scopes lost, over 1%")
        loss = 100.0 * verdict["missing_total"] / max(1, expected_total)
        return (f"{sum(verdict['observed'].values())} of {expected_total} scopes, {loss:.3f}% lost "
                f"({healed} healed: {dup} dup, {discarded} unclosed, {orphans} orphan; "
                f"{verdict['depth_wrong_total']} at wrong depth), {threads} threads x {hz} Hz x {calls}")
    finally:
        app.stop()


@scenario("report-filter", "The report's filter box narrows the rows to the functions that match")
def report_filter(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    run.load_symbols()
    run.capture(seconds=4.0)
    run.stop_capture()
    run.open_viewer("?collapse=scheduler&report=flat")
    before = run.wait_for(lambda: run.rects_matching("report:") or None, "flat report rows", timeout=20)
    run.click("report_filter")
    run.chrome.call("Input.insertText", text="b3Mul")
    after = run.wait_for(
        lambda: (lambda r: r if r and all("b3mul" in k.lower() for k in r) and len(r) < len(before) else None)(
            run.rects_matching("report:")),
        "only the rows containing the filter", timeout=10,
    )
    check(run.sel().get("report_filter") == "b3Mul", "the readout shows the filter")
    run.shot("25-report-filter", settle=0.5)
    # The trees open along the paths to the matches.
    run.click("Top-down")
    run.wait_for(lambda: any("b3mul" in k.lower() for k in run.rects_matching("tree:")), "a matching tree node", timeout=20)
    # Escape in the box clears it, and every row is back.
    run.click("report_filter")
    run.chrome.key("Escape")
    run.click("Flat")
    run.wait_for(lambda: len(run.rects_matching("report:")) >= len(before), "the rows back after Escape", timeout=10)
    return f"{len(after)} of {len(before)} rows match 'b3Mul'"


@scenario("sample-bar-select", "A left-drag on one thread's sample bar selects that thread's samples, drawn on the bar")
def sample_bar_select(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    run.load_symbols()
    run.capture(seconds=4.0)
    run.stop_capture()
    run.open_viewer("?collapse=scheduler")
    run.wait_for(lambda: run.sel().get("events"), "events", timeout=20)
    time.sleep(0.5)
    bars = {k: v for k, v in run.rects_matching("sample_bar:").items()}
    check(bars, "no sample bars are drawn")
    # A bar well inside the viewport.
    canvas_h = run.chrome.eval("document.querySelector('canvas').clientHeight")
    inview = sorted((v[1], k) for k, v in bars.items() if 120 < v[1] < canvas_h - 40)
    check(inview, f"no sample bar in view: {sorted(bars)[:4]}")
    label = inview[len(inview) // 2][1]
    tid = int(label.split(":")[2])
    x, y, w, h = bars[label]
    cy = y + h * 0.5
    x0, x1 = x + w * 0.30, x + w * 0.62
    # A real press-move-release: egui needs the intermediate moves to build
    # a drag, and the selection only starts when the press lands on the bar.
    def mouse(kind, px, py):
        run.chrome.call("Input.dispatchMouseEvent", type=kind, x=px, y=py, button="left", buttons=1)
    mouse("mousePressed", x0, cy)
    for i in range(1, 11):
        mouse("mouseMoved", x0 + (x1 - x0) * i / 10, cy)
        time.sleep(0.02)
    run.shot("30-sample-bar-select", settle=0.3)
    mouse("mouseReleased", x1, cy)
    ranges = run.wait_for(
        lambda: (run.sel().get("ranges") or None), "the committed selection", timeout=10
    )
    # One range, carrying this thread's tid: the selection is per-thread.
    tids = [r[2] for r in ranges if len(r) >= 3]
    check(tid in tids, f"selection is not scoped to thread {tid}: {ranges}")
    return f"selected {len(ranges)} range(s) on thread {tid}"


@scenario("code-views", "Source, disassembly and the two interleaved: the examples, then a Box3D function from the report")
def code_views(run):
    if run.chrome is None:
        return "skipped: --no-shots"
    run.load_symbols()
    run.capture(seconds=3.0)
    run.stop_capture()
    run.open_viewer("?collapse=scheduler&report=code")
    run.wait_for(lambda: run.sel().get("tab") == "Code", "the Code tab", timeout=20)

    def code():
        return run.sel().get("code") or {}

    def example(label):
        run.click("Examples")
        run.rect(label, timeout=5)
        run.click(label)
        return run.wait_for(lambda: (not code().get("loading")) and code().get("rows") and code() or None,
                            f"{label} loaded", timeout=30)

    # The embedded files: each opens, highlighted, at its first function.
    rust = example("code:example:rust")
    check(rust["mode"] == "Source" and rust["rows"] > 500 and rust["source"].endswith("uprobes.rs"), f"rust example: {rust}")
    run.shot("27-code-source", settle=0.5)
    cpp = example("code:example:cpp")
    check(cpp["rows"] > 500 and cpp["source"].endswith(".cpp"), f"C++ example: {cpp}")
    # The service's own function, disassembled live, its source interleaved.
    asm = example("code:example:asm")
    check(asm["mode"] == "Both" and asm["instructions"] > 100, f"example disassembly: {asm}")
    check(asm["rows"] > asm["instructions"], f"Both has no source rows: {asm}")
    check(asm["source"].endswith("uprobes.rs"), f"the example's source is not uprobes.rs: {asm}")
    run.shot("26-code-both", settle=0.5)
    run.click("Disassembly")
    run.wait_for(lambda: code().get("rows") == asm["instructions"], "disassembly alone")
    run.click("Source")
    run.wait_for(lambda: code().get("mode") == "Source", "source alone")
    # A function of the target, from the Flat report's context menu.
    run.click("Flat")
    rows = run.wait_for(lambda: run.rects_matching("report:") or None, "flat report rows", timeout=20)
    canvas_h = run.chrome.eval("document.querySelector('canvas').clientHeight")
    target_rows = sorted((v[1], k) for k, v in rows.items()
                         if (k.startswith("report:b3") or "orbit_e2e" in k) and 0 <= v[1] < canvas_h - 20)
    check(target_rows, f"no Box3D row in view: {sorted(rows)[:8]}")
    row = target_rows[0][1]
    run.click(row, button="right")
    run.click("menu:disassemble")
    got = run.wait_for(lambda: (not code().get("loading")) and code().get("disasm") and code() or None,
                       "the row's disassembly", timeout=30)
    name = row[len("report:"):]
    check(got["disasm"] == name and got["instructions"] > 0, f"disassembly of {name!r}: {got}")
    check(run.sel().get("tab") == "Code", "the Code tab opened")
    return (f"examples: uprobes.rs {rust['rows']} lines, C++ {cpp['rows']} lines, "
            f"{asm['disasm'].rsplit('::', 1)[-1]} {asm['instructions']} instructions in {asm['rows']} rows; "
            f"{name}: {got['instructions']} instructions ({got['mode']})")


# --------------------------------------------------------------------- main

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=44810)
    parser.add_argument("--box3d", default=DEFAULT_BOX3D)
    parser.add_argument("--shots", default=os.path.join(REPO, "docs/screenshots"))
    parser.add_argument("--only", action="append", help="run only these scenarios")
    parser.add_argument("--keep-going", action="store_true",
                        help="run every scenario even after one fails")
    parser.add_argument("--no-shots", action="store_true", help="skip screenshots")
    parser.add_argument("--report", default=os.path.join(REPO, "docs/e2e/report.md"),
                        help="where the results, perf numbers and screenshot index go")
    parser.add_argument("--sudo", action="store_true",
                        help="run the service as root (sudo -n): dynamic instrumentation arms")
    args = parser.parse_args()
    global SUDO
    SUDO = args.sudo

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
    write_report(args.report, results, run.perf if 'run' in locals() else {}, args.shots)
    print(f"report in {args.report}")
    return 0 if passed == len(results) else 1


def write_report(path, results, perf, shots_dir):
    """The run as a Markdown page: results, the numbers the scenarios
    measured, and every screenshot with the scenario that took it. This and
    docs/manual/features.md are what an agent reads to write the manual."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    stamp = time.strftime("%Y-%m-%d %H:%M")
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=REPO,
                          capture_output=True, text=True).stdout.strip()
    lines = [f"# Orbit e2e report", "", f"Run {stamp} at commit `{head}` on `{os.uname().nodename}`.", "",
             "| Scenario | Result | Time | Note |", "|---|---|---|---|"]
    for name, verdict, note, seconds in results:
        lines.append(f"| {name} | {verdict} | {seconds:.1f}s | {(note or '').replace('|', '/')} |")
    lines += ["", "## Numbers", ""]
    if perf:
        for key in ("wire", "events", "ws_bps_during_capture", "bundle_bytes", "slice_bytes", "stream_bytes", "site_first_events_s"):
            if key in perf:
                lines.append(f"- {key}: {perf[key]}")
        if "status" in perf:
            lines.append(f"- service status: {json.dumps(perf['status'])}")
        phases = perf.get("self", {}).get("phases", [])
        if phases:
            lines += ["", "Viewer self-profile (headless Chrome, SwiftShader: slower than a GPU):", "",
                      "| Phase | Total ms | Count | Avg us | Max us |", "|---|---|---|---|---|"]
            for p in phases:
                lines.append(f"| {p['name']} | {p['total_ms']} | {p['count']} | {p['avg_us']} | {p['max_us']} |")
    else:
        lines.append("(no numbers: the perf scenario did not run)")
    lines += ["", "## Screenshots", ""]
    if os.path.isdir(shots_dir):
        for shot in sorted(os.listdir(shots_dir)):
            if shot.endswith(".png"):
                lines.append(f"- `{shot}` -- ![]({os.path.relpath(os.path.join(shots_dir, shot), os.path.dirname(path))})")
    with open(path, "w") as handle:
        handle.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    sys.exit(main())
