# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Attach Orbit or perf to a generic child PID (no target-specific protocol)."""

from __future__ import annotations

import json
import os
import re
import shutil
import socket
import subprocess
import time
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
ORBIT_SERVICE = ROOT / "rust" / "crates" / "orbit-service" / "target" / "release" / "orbit-service"
ORBIT_POD_DUMP = ROOT / "rust" / "target" / "release" / "orbit-pod-dump"


def find_orbit_service() -> Path | None:
    for candidate in (
        ORBIT_SERVICE,
        ROOT / "rust" / "crates" / "orbit-service" / "target" / "x86_64-unknown-linux-musl" / "release" / "orbit-service",
        Path(shutil.which("orbit-service") or ""),
    ):
        if candidate and candidate.is_file():
            return candidate
    return None


def relax_perf_paranoid(target: int = 1) -> dict[str, Any]:
    path = Path("/proc/sys/kernel/perf_event_paranoid")
    current = path.read_text().strip() if path.exists() else None
    info: dict[str, Any] = {"before": current, "after": current, "changed": False}
    try:
        before = int(current) if current is not None else None
    except ValueError:
        before = None
    if before is None or before <= target:
        return info
    if subprocess.run(["sudo", "-n", "true"], capture_output=True).returncode != 0:
        info["reason"] = "sudo -n not available; capture may have 0 samples"
        return info
    proc = subprocess.run(
        ["sudo", "-n", "sysctl", f"kernel.perf_event_paranoid={target}"],
        text=True,
        capture_output=True,
        check=False,
    )
    info["sysctl"] = (proc.stdout + proc.stderr).strip()
    info["after"] = path.read_text().strip() if path.exists() else None
    info["changed"] = info["after"] != current
    return info


def _http_json(url: str, method: str = "GET", payload: Any | None = None, timeout: float = 60.0) -> Any:
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        body = response.read()
    if not body:
        return None
    if body[:1] in (b"{", b"["):
        return json.loads(body)
    return body


def _wait_http(url: str, timeout: float = 30.0) -> None:
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            _http_json(url, timeout=2.0)
            return
        except Exception as error:  # noqa: BLE001
            last = error
            time.sleep(0.2)
    raise RuntimeError(f"service never answered {url}: {last}")


def _free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def _merged_env(extra: dict[str, str] | None) -> dict[str, str]:
    env = os.environ.copy()
    if extra:
        env.update(extra)
    return env


def _spawn_workload(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None,
) -> subprocess.Popen[str]:
    return subprocess.Popen(
        cmd,
        cwd=cwd,
        env=_merged_env(env),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def _demangle(name: str) -> str:
    if not name.startswith("_Z"):
        return name
    filt = shutil.which("c++filt")
    if not filt:
        return name
    try:
        out = subprocess.run([filt, "-n", name], capture_output=True, text=True, check=False)
        return (out.stdout or "").strip() or name
    except Exception:  # noqa: BLE001
        return name


def capture_orbit_http(
    orbit: Path,
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None,
    out_dir: Path,
    duration_ms: int,
) -> dict[str, Any]:
    port = _free_port()
    log_path = out_dir / "orbit-service.log"
    log_file = open(log_path, "w", encoding="utf-8")
    service = subprocess.Popen(
        [str(orbit), "--host", "127.0.0.1", "--serve", str(port)],
        stdout=log_file,
        stderr=subprocess.STDOUT,
        text=True,
    )
    base = f"http://127.0.0.1:{port}"
    target = None
    try:
        _wait_http(base + "/api/status", timeout=40.0)
        target = _spawn_workload(cmd, cwd=cwd, env=env)
        time.sleep(0.2)
        symbols: dict[str, Any] = {}
        symbol_error = None
        try:
            _http_json(base + "/api/symbols/load", method="POST", payload={"pid": target.pid})
            deadline = time.time() + 40
            last: dict[str, Any] = {}
            while time.time() < deadline:
                last = _http_json(base + f"/api/symbols/status?pid={target.pid}") or {}
                if last.get("status") == "ready":
                    symbols = last
                    break
                if last.get("status") == "error":
                    raise RuntimeError(str(last))
                time.sleep(0.3)
            else:
                symbol_error = f"symbols never ready: {last}"
        except Exception as error:  # noqa: BLE001
            symbol_error = str(error)
        _http_json(base + "/api/capture/start", method="POST", payload={"pid": target.pid})
        time.sleep(max(0.5, duration_ms / 1000.0))
        if target.poll() is None:
            target.terminate()
            try:
                target.wait(timeout=5)
            except subprocess.TimeoutExpired:
                target.kill()
        _http_json(base + "/api/capture/stop", method="POST", payload={})
        time.sleep(0.4)
        report = _http_json(
            base + "/api/sampling/report?start_ns=0&end_ns=18446744073709551615",
            timeout=30.0,
        )
        hotspots = []
        if isinstance(report, dict):
            for row in (report.get("functions") or [])[:20]:
                raw = row.get("name") or "??"
                hotspots.append(
                    {
                        "symbol": _demangle(raw),
                        "symbol_raw": raw,
                        "self": row.get("self"),
                        "self_percent": row.get("self_percent"),
                    }
                )
        return {
            "backend": "orbit-http",
            "ok": bool(hotspots) and bool((report or {}).get("samples")),
            "samples": (report or {}).get("samples") if isinstance(report, dict) else None,
            "hotspots": hotspots,
            "symbols": symbols,
            "symbol_error": symbol_error,
            "pid": target.pid if target else None,
            "command": f"{orbit} --serve ; capture pid of: {cmd}",
            "service_log": str(log_path),
            "note": (
                "Generic PID attach. Serve-mode on this VM has historically "
                "opened rings but recorded 0 callstack samples."
            ),
        }
    finally:
        if target and target.poll() is None:
            target.kill()
        if service.poll() is None:
            service.terminate()
            try:
                service.wait(timeout=8)
            except subprocess.TimeoutExpired:
                service.kill()
        log_file.close()


def capture_orbit_file(
    orbit: Path,
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None,
    out_dir: Path,
    duration_ms: int,
) -> dict[str, Any]:
    pod = out_dir / "capture.pod"
    target = _spawn_workload(cmd, cwd=cwd, env=env)
    try:
        time.sleep(0.2)
        svc = subprocess.Popen(
            [str(orbit), "--pid", str(target.pid), "--duration-ms", str(duration_ms + 2000), "--out", str(pod)],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        time.sleep(duration_ms / 1000.0)
        if target.poll() is None:
            target.terminate()
            try:
                target.wait(timeout=5)
            except subprocess.TimeoutExpired:
                target.kill()
        try:
            out, _ = svc.communicate(timeout=20)
        except subprocess.TimeoutExpired:
            svc.kill()
            out, _ = svc.communicate(timeout=5)
        (out_dir / "orbit-file.log").write_text(out or "")
        dump_text = ""
        if ORBIT_POD_DUMP.is_file() and pod.exists():
            dump = subprocess.run(
                [str(ORBIT_POD_DUMP), str(pod), "--top", "20"],
                capture_output=True,
                text=True,
                check=False,
            )
            dump_text = dump.stdout + dump.stderr
            (out_dir / "orbit-pod-dump.txt").write_text(dump_text)
        samples = 0
        match = re.search(r"captured (\d+) samples", out or "")
        if match:
            samples = int(match.group(1))
        hotspots: list[dict[str, Any]] = []
        block = re.compile(r"^\s+(\d+)\s+samples\s+([0-9.]+)%", re.M)
        for row in block.finditer(dump_text):
            hotspots.append({"self": int(row.group(1)), "self_percent": float(row.group(2)), "symbol": "(see dump)"})
            if len(hotspots) >= 15:
                break
        return {
            "backend": "orbit-file",
            "ok": samples > 0,
            "samples": samples,
            "hotspots": hotspots,
            "pid": target.pid,
            "pod": str(pod) if pod.exists() else None,
            "command": f"{orbit} --pid {target.pid} --duration-ms {duration_ms} --out {pod}",
            "note": "File-mode samples one tid (the launched process leader).",
            "dump_tail": dump_text[-800:],
        }
    finally:
        if target.poll() is None:
            target.kill()


def capture_perf(cmd: list[str], *, cwd: Path, env: dict[str, str] | None, out_dir: Path) -> dict[str, Any]:
    perf = shutil.which("perf")
    if not perf:
        return {"backend": "perf", "ok": False, "skipped": True, "reason": "perf CLI is not installed"}
    data = out_dir / "perf.data"
    record = subprocess.run(
        [perf, "record", "--call-graph", "dwarf", "-o", str(data), "--", *cmd],
        cwd=cwd,
        env=_merged_env(env),
        capture_output=True,
        text=True,
        check=False,
        timeout=180,
    )
    report = subprocess.run(
        [perf, "report", "--stdio", "--demangle", "-i", str(data)],
        capture_output=True,
        text=True,
        check=False,
    )
    (out_dir / "perf-report.txt").write_text(report.stdout)
    hotspots = []
    row = re.compile(r"^\s+([0-9.]+)%\s+.*\[.\]\s+(.+)$")
    for line in report.stdout.splitlines():
        match = row.match(line)
        if match:
            hotspots.append({"self_percent": float(match.group(1)), "symbol": match.group(2).strip()})
        if len(hotspots) >= 20:
            break
    return {
        "backend": "perf",
        "ok": bool(hotspots),
        "hotspots": hotspots,
        "command": f"perf record -- {' '.join(cmd)}",
        "returncode": record.returncode,
    }


def run_profile(
    mode: str,
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None,
    out_dir: Path,
    duration_ms: int,
) -> dict[str, Any]:
    out_dir.mkdir(parents=True, exist_ok=True)
    attempts: list[dict[str, Any]] = []
    orbit = find_orbit_service()
    paranoid = relax_perf_paranoid(1)

    def want(name: str) -> bool:
        return mode in ("auto", name, "all")

    if want("orbit") and orbit:
        try:
            result = capture_orbit_http(
                orbit, cmd, cwd=cwd, env=env, out_dir=out_dir, duration_ms=duration_ms
            )
        except Exception as error:  # noqa: BLE001
            result = {"backend": "orbit-http", "ok": False, "error": str(error)}
        attempts.append(result)
        if result.get("ok") and mode != "all":
            result["attempts"] = attempts
            result["perf_event_paranoid"] = paranoid
            return result
        try:
            file_result = capture_orbit_file(
                orbit, cmd, cwd=cwd, env=env, out_dir=out_dir, duration_ms=duration_ms
            )
        except Exception as error:  # noqa: BLE001
            file_result = {"backend": "orbit-file", "ok": False, "error": str(error)}
        attempts.append(file_result)
        if file_result.get("ok") and mode != "all":
            chosen = dict(file_result)
            chosen["attempts"] = attempts
            chosen["perf_event_paranoid"] = paranoid
            return chosen
    elif want("orbit"):
        attempts.append({"backend": "orbit", "ok": False, "skipped": True, "reason": "orbit-service not built"})

    if want("perf"):
        attempts.append(capture_perf(cmd, cwd=cwd, env=env, out_dir=out_dir))

    chosen = next((a for a in attempts if a.get("ok")), attempts[-1] if attempts else {"ok": False, "skipped": True})
    chosen = dict(chosen)
    chosen["attempts"] = [{k: v for k, v in a.items() if k != "attempts"} for a in attempts]
    chosen["perf_event_paranoid"] = paranoid
    if not attempts:
        chosen = {
            "backend": "none",
            "ok": False,
            "skipped": True,
            "reason": "capture disabled",
            "hotspots": [],
            "perf_event_paranoid": paranoid,
        }
    return chosen
