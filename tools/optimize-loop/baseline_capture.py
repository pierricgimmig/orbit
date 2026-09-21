#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Takes the *baseline capture* of an optimization target with Orbit:
sampling on every thread plus a handful of coarse hooks that outline the
program's structure, then exports the stream (to embed in the baseline post),
the bundle (the record) and the sampling report (the hotspot table).

    python3 tools/optimize-loop/baseline_capture.py <binary> <out_prefix> <seconds> \
        <hook>[,<hook>...] -- <args...>

Hook names are substrings matched against the target's symbol table (a
mangled fragment such as `6Engine2goE` is the precise way to name one
overload). Pick coarse functions -- ones called thousands of times, not
millions -- or the rings drop and the numbers are wrong. Hooks need
CAP_SYS_ADMIN: with ORBIT_SUDO_WRAPPER (default /usr/local/bin/orbit-service-sudo)
present the service runs through it; otherwise it runs unprivileged and the
capture is sampling only. Outputs: <prefix>.orbit.stream, .orbit.zip,
.report.json, .capture.json, .service.log."""
import json, os, socket, subprocess, sys, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
ORBIT = os.environ.get("ORBIT_SERVICE", os.path.join(REPO, "rust/crates/orbit-service/target/release/orbit-service"))
WRAP = os.environ.get("ORBIT_SUDO_WRAPPER", "/usr/local/bin/orbit-service-sudo")


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def jget(base, path, method="GET", payload=None, timeout=120):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(base + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        b = r.read()
    return json.loads(b) if b[:1] in (b"{", b"[") else b


def wait_http(base, timeout=40):
    end = time.time() + timeout
    while time.time() < end:
        try:
            jget(base, "/api/status"); return
        except Exception:
            time.sleep(0.2)
    raise RuntimeError("service never came up")


def main():
    binary, prefix, seconds, hooks = sys.argv[1], sys.argv[2], float(sys.argv[3]), sys.argv[4].split(",")
    args = sys.argv[sys.argv.index("--") + 1:]
    port = free_port()
    log = open(prefix + ".service.log", "w")
    privileged = os.path.exists(WRAP)
    cmd = (["sudo", "-n", "--", WRAP, ORBIT] if privileged else [ORBIT]) + ["--host", "127.0.0.1", "--serve", str(port)]
    svc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, text=True)
    base = f"http://127.0.0.1:{port}"
    result = {"binary": binary, "args": args, "seconds": seconds, "privileged": privileged}
    if not privileged and hooks != [""]:
        print(f"no sudo wrapper at {WRAP}: running unprivileged, sampling only (hooks ignored)", file=sys.stderr)
        hooks = []
    try:
        wait_http(base)
        tgt = subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        time.sleep(0.4)
        pid = tgt.pid
        jget(base, "/api/symbols/load", "POST", {"pid": pid})
        end = time.time() + 90
        while time.time() < end:
            s = jget(base, f"/api/symbols/status?pid={pid}")
            if s.get("status") == "ready":
                break
            if s.get("status") == "error":
                raise RuntimeError(s)
            time.sleep(0.4)
        resolved = []
        for h in [h for h in hooks if h]:
            hits = jget(base, f"/api/functions/search?pid={pid}&q={h}&limit=8").get("functions", [])
            if not hits:
                print(f"no symbol for {h!r}", file=sys.stderr); continue
            resolved.append({"query": h, "function_id": hits[0]["function_id"], "name": hits[0]["name"],
                             "candidates": [x["name"] for x in hits[:4]]})
        result["hooks"] = resolved
        jget(base, "/api/capture/start", "POST",
             {"pid": pid, "sampling": True,
              "instrumented_functions": [{"function_id": r["function_id"]} for r in resolved],
              "dynamic_instrumentation_method": "kernel_uprobes"})
        t0 = time.time()
        time.sleep(seconds)
        if tgt.poll() is None:
            tgt.terminate()
        try:
            tgt.wait(timeout=5)
        except Exception:
            tgt.kill()
        jget(base, "/api/capture/stop", "POST", {})
        time.sleep(0.8)
        result["wall_s"] = round(time.time() - t0, 1)
        result["status"] = jget(base, "/api/status")
        rep = jget(base, "/api/sampling/report?start_ns=0&end_ns=18446744073709551615", timeout=60)
        json.dump(rep, open(prefix + ".report.json", "w"))
        result["samples"] = rep.get("samples")
        result["top"] = [(round(r.get("self_percent", 0), 2), round(r.get("total_percent", r.get("inclusive_percent", 0)) or 0, 2),
                          r.get("name", "")) for r in (rep.get("functions") or [])[:40]]
        for fmt, ext in (("stream", ".orbit.stream"), ("bundle", ".orbit.zip")):
            body = jget(base, f"/api/capture/export?format={fmt}", timeout=180)
            if isinstance(body, (bytes, bytearray)) and body:
                open(prefix + ext, "wb").write(body)
                result[fmt + "_bytes"] = len(body)
    finally:
        try:
            svc.terminate(); svc.wait(timeout=8)
        except Exception:
            pass
    json.dump(result, open(prefix + ".capture.json", "w"), indent=1, default=str)
    print(json.dumps({k: v for k, v in result.items() if k not in ("top", "status")}, default=str))
    print("instrumentation:", (result.get("status") or {}).get("instrumentation", "")[:160])
    for s, t, n in result.get("top", [])[:12]:
        print(f"  self {s:6.2f}%  total {t:6.2f}%  {n[:90]}")


if __name__ == "__main__":
    main()
