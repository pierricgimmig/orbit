#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Phase 2 harness: Stockfish speedtest, repeated bench, perft, and profiling.

This is the poster-project benchmarking suite. It does not change Stockfish
sources and does not run the agent loop. It writes JSON + Markdown summaries
and can print before/after deltas against a previous summary.

Usage (from the Orbit repo root):

  python3 tools/stockfish-orbit-loop/run_suite.py
  python3 tools/stockfish-orbit-loop/run_suite.py --baseline path/to/summary.json
  python3 tools/stockfish-orbit-loop/run_suite.py --compare old.json new.json
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
STOCKFISH_SRC = ROOT / "stockfish" / "src"
STOCKFISH_BIN = STOCKFISH_SRC / "stockfish"
OFFICIAL_PERFT = ROOT / "stockfish" / "tests" / "perft.sh"
PINNED_SHA = "031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3"
ORBIT_SERVICE = ROOT / "rust" / "crates" / "orbit-service" / "target" / "release" / "orbit-service"
ORBIT_POD_DUMP = ROOT / "rust" / "target" / "release" / "orbit-pod-dump"

# Official `speedtest` defaults (see stockfish/src/benchmark.cpp).
SPEEDTEST_HASH_PER_THREAD_MIB = 128
SPEEDTEST_OFFICIAL_SECONDS = 150

# Secondary bench: the upstream fingerprint command, repeated for a mean.
BENCH_ARGS = ["16", "1", "13", "default", "depth"]

# Official perft.sh positions at reduced depth (published node counts).
# Full official depths take a long time (startpos d7 is 3.2e9 nodes).
SMOKE_PERFT = [
    # (uci_position, depth, expected, chess960)
    ("startpos", 4, 197281, False),
    (
        "fen r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq -",
        3,
        97862,
        False,
    ),
    ("fen 8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - -", 4, 43238, False),
    (
        "fen r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        3,
        9467,
        False,
    ),
    (
        "fen rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        3,
        62379,
        False,
    ),
    # From official tests/perft.sh (chess960, already a short case).
    (
        "fen rr6/2kpp3/1ppn2p1/p2b1q1p/P4P1P/1PNN2P1/2PP4/1K2R2R b E - 1 20",
        2,
        1438,
        True,
    ),
]


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    timeout: float | None = None,
    check: bool = True,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=cwd,
        timeout=timeout,
        check=check,
        text=True,
        capture_output=True,
        env=env,
    )


def git_sha(repo: Path) -> str:
    try:
        return run(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "unknown"


def nproc() -> int:
    return os.cpu_count() or 1


def demangle(name: str) -> str:
    if not name.startswith("_Z"):
        return name
    filt = shutil.which("c++filt")
    if not filt:
        return name
    try:
        out = run([filt, "-n", name], check=False).stdout.strip()
        return out or name
    except Exception:  # noqa: BLE001
        return name


def relax_perf_paranoid(target: int = 1) -> dict[str, Any]:
    """Lower kernel.perf_event_paranoid so orbit-service can sample.

    At the cloud VM default (2), this Orbit's serve-mode sampler opened no
    rings against Stockfish. File-mode and serve-mode both work at 1 for
    same-user processes. Requires passwordless sudo; otherwise we record
    the current value and continue.
    """
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


def mean_stdev(values: list[float]) -> tuple[float, float]:
    if not values:
        return (0.0, 0.0)
    if len(values) == 1:
        return (values[0], 0.0)
    return (statistics.mean(values), statistics.stdev(values))


def pct_delta(old: float | None, new: float | None) -> float | None:
    if old is None or new is None or old == 0:
        return None
    return 100.0 * (new - old) / old


def parse_bench(text: str) -> dict[str, int]:
    def grab(label: str) -> int:
        match = re.search(rf"{re.escape(label)}\s*:\s*(\d+)", text)
        if not match:
            raise ValueError(f"bench output missing {label!r}")
        return int(match.group(1))

    return {
        "time_ms": grab("Total time (ms)"),
        "nodes": grab("Nodes searched"),
        "nps": grab("Nodes/second"),
    }


def parse_speedtest(text: str) -> dict[str, Any]:
    def grab(label: str, typ=float):
        match = re.search(rf"{re.escape(label)}\s*:\s*([0-9.]+)", text)
        if not match:
            raise ValueError(f"speedtest output missing {label!r}")
        return typ(match.group(1))

    filled = None
    match = re.search(r"Filled invocation\s*:\s*speedtest\s+(.+)", text)
    if match:
        filled = match.group(1).strip()
    return {
        "nps": int(grab("Nodes/second", float)),
        "nodes": int(grab("Total nodes searched", float)),
        "time_s": float(grab("Total search time [s]")),
        "filled_invocation": filled,
        "raw_tail": "\n".join(text.strip().splitlines()[-20:]),
    }


# ---------------------------------------------------------------------------
# Stockfish runs
# ---------------------------------------------------------------------------


def ensure_stockfish(binary: Path) -> None:
    if not binary.is_file() or not os.access(binary, os.X_OK):
        sys.exit(
            f"missing {binary}\n"
            "Build it first:  ./docs/stockfish-orbit-loop/build.sh --build"
        )


def stockfish_compiler(binary: Path) -> str:
    proc = run([str(binary), "compiler"])
    return (proc.stdout + proc.stderr).strip()


def run_speedtest(binary: Path, threads: int, hash_mib: int, seconds: int, log: Path) -> dict[str, Any]:
    cmd = [str(binary), "speedtest", str(threads), str(hash_mib), str(seconds)]
    proc = run(cmd, timeout=seconds + 120)
    text = proc.stdout + proc.stderr
    log.write_text(text)
    parsed = parse_speedtest(text)
    parsed["command"] = cmd
    parsed["threads"] = threads
    parsed["hash_mib"] = hash_mib
    parsed["requested_seconds"] = seconds
    return parsed


def run_bench_once(binary: Path) -> dict[str, int]:
    cmd = [str(binary), "bench", *BENCH_ARGS]
    proc = run(cmd, timeout=120)
    return parse_bench(proc.stdout + proc.stderr)


def run_benches(binary: Path, iterations: int, log: Path) -> dict[str, Any]:
    rows = []
    texts = []
    for i in range(iterations):
        cmd = [str(binary), "bench", *BENCH_ARGS]
        proc = run(cmd, timeout=120)
        text = proc.stdout + proc.stderr
        texts.append(f"===== iteration {i + 1}/{iterations} =====\n{text}")
        parsed = parse_bench(text)
        parsed["iteration"] = i + 1
        rows.append(parsed)
        print(f"  bench {i + 1}/{iterations}: {parsed['nps']} nps  ({parsed['time_ms']} ms)", flush=True)
    log.write_text("\n".join(texts))
    nps = [float(r["nps"]) for r in rows]
    times = [float(r["time_ms"]) for r in rows]
    nps_mean, nps_stdev = mean_stdev(nps)
    time_mean, time_stdev = mean_stdev(times)
    return {
        "command": [str(binary), "bench", *BENCH_ARGS],
        "iterations": iterations,
        "nodes_searched": rows[0]["nodes"] if rows else None,
        "nps": {
            "mean": nps_mean,
            "stdev": nps_stdev,
            "min": min(nps) if nps else None,
            "max": max(nps) if nps else None,
            "values": [int(v) for v in nps],
        },
        "time_ms": {
            "mean": time_mean,
            "stdev": time_stdev,
            "min": min(times) if times else None,
            "max": max(times) if times else None,
            "values": [int(v) for v in times],
        },
        "runs": rows,
    }


def stockfish_perft(binary: Path, position: str, depth: int, chess960: bool) -> int:
    commands = []
    if chess960:
        commands.append("setoption name UCI_Chess960 value true")
    commands.append(f"position {position}")
    commands.append(f"go perft {depth}")
    commands.append("quit")
    proc = subprocess.run(
        [str(binary)],
        input="\n".join(commands) + "\n",
        text=True,
        capture_output=True,
        timeout=180,
        check=True,
    )
    text = proc.stdout + proc.stderr
    match = re.search(r"Nodes searched:\s*(\d+)", text)
    if not match:
        raise RuntimeError(f"perft produced no node count:\n{text[-500:]}")
    return int(match.group(1))


def run_perft_smoke(binary: Path, log: Path) -> dict[str, Any]:
    tests = []
    all_ok = True
    lines = ["smoke perft (official positions, reduced depth)", ""]
    for position, depth, expected, chess960 in SMOKE_PERFT:
        got = stockfish_perft(binary, position, depth, chess960)
        ok = got == expected
        all_ok = all_ok and ok
        label = position if len(position) < 48 else position[:45] + "..."
        status = "OK" if ok else f"FAILED got {got} expected {expected}"
        print(f"  perft depth {depth}: {label} ... {status}", flush=True)
        lines.append(f"{status}: depth {depth} expected {expected} got {got}  {position}")
        tests.append(
            {
                "position": position,
                "depth": depth,
                "expected": expected,
                "got": got,
                "chess960": chess960,
                "ok": ok,
            }
        )
    log.write_text("\n".join(lines) + "\n")
    return {
        "mode": "smoke",
        "passed": all_ok,
        "command": "python stockfish_perft on official positions (reduced depth)",
        "note": "Full official tests/perft.sh is --perft official (needs expect; many minutes).",
        "tests": tests,
    }


def run_perft_official(log: Path) -> dict[str, Any]:
    if not shutil.which("expect"):
        return {
            "mode": "official",
            "passed": False,
            "command": str(OFFICIAL_PERFT),
            "error": "expect is not installed (apt install expect)",
            "tests": [],
        }
    if not OFFICIAL_PERFT.is_file():
        return {
            "mode": "official",
            "passed": False,
            "command": str(OFFICIAL_PERFT),
            "error": "stockfish/tests/perft.sh is missing",
            "tests": [],
        }
    print("  running official tests/perft.sh (this can take a long time)...", flush=True)
    proc = subprocess.run(
        ["bash", str(OFFICIAL_PERFT)],
        cwd=STOCKFISH_SRC,
        text=True,
        capture_output=True,
        timeout=4 * 3600,
        check=False,
    )
    text = proc.stdout + proc.stderr
    log.write_text(text)
    passed = proc.returncode == 0 and "Some tests failed" not in text
    return {
        "mode": "official",
        "passed": passed,
        "command": f"cd {STOCKFISH_SRC} && bash {OFFICIAL_PERFT}",
        "returncode": proc.returncode,
        "tail": "\n".join(text.strip().splitlines()[-40:]),
        "tests": [],
    }


# ---------------------------------------------------------------------------
# Profiling backends (Orbit HTTP, Orbit file, perf CLI)
# ---------------------------------------------------------------------------


def http_json(url: str, method: str = "GET", payload: Any | None = None, timeout: float = 60.0) -> Any:
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


def wait_http(url: str, timeout: float = 30.0) -> None:
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            http_json(url, timeout=2.0)
            return
        except Exception as error:  # noqa: BLE001 — retried until deadline
            last = error
            time.sleep(0.2)
    raise RuntimeError(f"service never answered {url}: {last}")


def free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def start_uci(binary: Path) -> subprocess.Popen[str]:
    proc = subprocess.Popen(
        [str(binary)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    assert proc.stdin and proc.stdout
    # Version banner, then we own the UCI session.
    proc.stdout.readline()
    return proc


def uci_command(proc: subprocess.Popen[str], command: str, until: str, timeout: float = 180) -> str:
    assert proc.stdin and proc.stdout
    proc.stdin.write(command + "\n")
    proc.stdin.flush()
    lines = []
    deadline = time.time() + timeout
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            break
        lines.append(line)
        if until in line:
            time.sleep(0.05)
            break
    return "".join(lines)


def uci_bench(proc: subprocess.Popen[str], args: list[str] | None = None) -> str:
    cmd = "bench" if not args else "bench " + " ".join(args)
    return uci_command(proc, cmd, "Nodes/second")


def uci_movetime(proc: subprocess.Popen[str], ms: int) -> str:
    assert proc.stdin
    proc.stdin.write("ucinewgame\nposition startpos\n")
    proc.stdin.flush()
    return uci_command(proc, f"go movetime {ms}", "bestmove", timeout=ms / 1000 + 30)


class OrbitHttpCapture:
    """This Orbit (pierricgimmig/orbit) via orbit-service HTTP API.

    Serve mode samples every thread of the target and can symbolize names
    through GET /api/sampling/report. File-mode --pid only opens one tid.
    """

    def __init__(self, binary: Path, out_dir: Path):
        self.binary = binary
        self.out_dir = out_dir
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.log_path = out_dir / "orbit-service.log"
        self.proc: subprocess.Popen[str] | None = None
        self.log_file = None

    def start(self) -> None:
        self.log_file = open(self.log_path, "w", encoding="utf-8")
        self.proc = subprocess.Popen(
            [str(self.binary), "--host", "127.0.0.1", "--serve", str(self.port)],
            stdout=self.log_file,
            stderr=subprocess.STDOUT,
            text=True,
        )
        wait_http(self.base + "/api/status", timeout=40.0)

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        if self.log_file:
            self.log_file.close()

    def load_symbols(self, pid: int, timeout: float = 60.0) -> dict[str, Any]:
        http_json(self.base + "/api/symbols/load", method="POST", payload={"pid": pid})
        deadline = time.time() + timeout
        last: dict[str, Any] = {}
        while time.time() < deadline:
            last = http_json(self.base + f"/api/symbols/status?pid={pid}") or {}
            if last.get("status") == "ready":
                return last
            if last.get("status") == "error":
                raise RuntimeError(f"symbol load failed: {last}")
            time.sleep(0.3)
        raise RuntimeError(f"symbols never became ready: {last}")

    def capture_bench(self, stockfish: Path) -> dict[str, Any]:
        target = start_uci(stockfish)
        try:
            symbols = {}
            symbol_error = None
            try:
                symbols = self.load_symbols(target.pid)
            except Exception as error:  # noqa: BLE001
                symbol_error = str(error)
            http_json(
                self.base + "/api/capture/start",
                method="POST",
                payload={"pid": target.pid},
            )
            # Rings finish opening after start returns; give them a beat.
            time.sleep(1.0)
            # A few seconds of search so samples land in evaluate/search, not just init.
            bench_text = uci_movetime(target, 4000)
            time.sleep(0.3)
            http_json(self.base + "/api/capture/stop", method="POST", payload={})
            time.sleep(0.8)
            report = http_json(
                self.base + "/api/sampling/report?start_ns=0&end_ns=18446744073709551615",
                timeout=30.0,
            )
            status = http_json(self.base + "/api/status") or {}
            hotspots = []
            if isinstance(report, dict):
                for row in (report.get("functions") or [])[:20]:
                    raw = row.get("name") or "??"
                    hotspots.append(
                        {
                            "symbol": demangle(raw),
                            "symbol_raw": raw,
                            "module": row.get("module") or "",
                            "self": row.get("self"),
                            "inclusive": row.get("inclusive"),
                            "self_percent": row.get("self_percent"),
                            "inclusive_percent": row.get("inclusive_percent"),
                        }
                    )
            bundle_path = None
            try:
                raw = http_json(self.base + "/api/capture/export?format=bundle", timeout=60.0)
                if isinstance(raw, (bytes, bytearray)) and raw:
                    bundle_path = str(self.out_dir / "capture.orbit.zip")
                    Path(bundle_path).write_bytes(raw)
            except Exception:  # noqa: BLE001 — export is optional
                bundle_path = None
            (self.out_dir / "orbit-report.json").write_text(
                json.dumps(report, indent=2, default=str) + "\n"
            )
            return {
                "backend": "orbit-http",
                "ok": bool(hotspots) and bool((report or {}).get("samples")),
                "command": (
                    f"{self.binary} --host 127.0.0.1 --serve {self.port} ; "
                    f"POST /api/capture/start pid={target.pid} ; "
                    "stockfish go movetime 4000"
                ),
                "samples": (report or {}).get("samples") if isinstance(report, dict) else None,
                "events_live": status.get("events_live"),
                "symbols": symbols,
                "symbol_error": symbol_error,
                "hotspots": hotspots,
                "bundle": bundle_path,
                "service_log": str(self.log_path),
                "bench_snippet": bench_text[-400:],
            }
        finally:
            if target.poll() is None:
                try:
                    if target.stdin:
                        target.stdin.write("quit\n")
                        target.stdin.flush()
                    target.wait(timeout=5)
                except Exception:  # noqa: BLE001
                    target.kill()


def parse_perf_report(text: str) -> list[dict[str, Any]]:
    hotspots = []
    # Typical: "    12.34%  stockfish  stockfish  [.] Search::..."
    row = re.compile(
        r"^\s+([0-9.]+)%\s+\S+\s+\S+\s+\[.\]\s+(.+?)\s*$"
    )
    for line in text.splitlines():
        match = row.match(line)
        if not match:
            continue
        hotspots.append(
            {
                "symbol": match.group(2).strip(),
                "self_percent": float(match.group(1)),
            }
        )
        if len(hotspots) >= 20:
            break
    if not hotspots:
        # Fallback: "12.34%  [.] symbol"
        loose = re.compile(r"^\s+([0-9.]+)%\s+.*\[.\]\s+(.+)$")
        for line in text.splitlines():
            match = loose.match(line)
            if match:
                hotspots.append(
                    {"symbol": match.group(2).strip(), "self_percent": float(match.group(1))}
                )
            if len(hotspots) >= 20:
                break
    return hotspots


def capture_perf(stockfish: Path, out_dir: Path) -> dict[str, Any]:
    perf = shutil.which("perf")
    if not perf:
        return {
            "backend": "perf",
            "ok": False,
            "skipped": True,
            "reason": "perf CLI is not installed on this host",
        }
    data = out_dir / "perf.data"
    record_cmd = [
        perf,
        "record",
        "--call-graph",
        "dwarf",
        "-o",
        str(data),
        "--",
        str(stockfish),
        "bench",
        *BENCH_ARGS,
    ]
    rec = subprocess.run(record_cmd, text=True, capture_output=True, timeout=180, check=False)
    (out_dir / "perf-record.log").write_text(rec.stdout + rec.stderr)
    if rec.returncode != 0 and not data.exists():
        return {
            "backend": "perf",
            "ok": False,
            "command": record_cmd,
            "reason": (rec.stderr or rec.stdout)[-500:],
        }
    report_cmd = [
        perf,
        "report",
        "--stdio",
        "--no-children",
        "--demangle",
        "-i",
        str(data),
        "--percent-limit",
        "0.5",
    ]
    rep = subprocess.run(report_cmd, text=True, capture_output=True, timeout=120, check=False)
    (out_dir / "perf-report.txt").write_text(rep.stdout + rep.stderr)
    hotspots = parse_perf_report(rep.stdout)
    return {
        "backend": "perf",
        "ok": bool(hotspots),
        "command": " ".join(record_cmd),
        "report_command": " ".join(report_cmd),
        "hotspots": hotspots,
        "perf_data": str(data),
    }


def parse_maps(text: str) -> list[dict[str, Any]]:
    maps = []
    for line in text.splitlines():
        parts = line.split()
        if len(parts) < 5:
            continue
        rng, perms, offset = parts[0], parts[1], parts[2]
        path = parts[5] if len(parts) > 5 else ""
        start_s, end_s = rng.split("-")
        maps.append(
            {
                "start": int(start_s, 16),
                "end": int(end_s, 16),
                "offset": int(offset, 16),
                "perms": perms,
                "path": path,
            }
        )
    return maps


def runtime_to_file(pc: int, maps: list[dict[str, Any]]) -> tuple[str | None, int | None]:
    for mapping in maps:
        if mapping["start"] <= pc < mapping["end"] and "x" in mapping["perms"]:
            return mapping["path"], pc - mapping["start"] + mapping["offset"]
    return None, None


def symbolize_pcs(stockfish: Path, maps_text: str, pcs: list[int]) -> dict[int, str]:
    maps = parse_maps(maps_text)
    out: dict[int, str] = {}
    for pc in pcs:
        path, elf_addr = runtime_to_file(pc, maps)
        binary = path if path and Path(path).is_file() else str(stockfish)
        if elf_addr is None:
            out[pc] = f"{pc:#x}"
            continue
        proc = run(
            ["addr2line", "-e", binary, "-f", "-C", "-p", f"{elf_addr:#x}"],
            check=False,
        )
        name = proc.stdout.strip()
        if not name or name.startswith("??"):
            out[pc] = f"{Path(binary).name}+{elf_addr:#x}"
        else:
            # "foo at src.cpp:10" → keep the function, drop "?" file.
            out[pc] = name.split(" at ")[0].strip()
    return out


def hotspots_from_pod_dump(dump_text: str, symbols: dict[int, str]) -> list[dict[str, Any]]:
    """Aggregate leaf frames from orbit-pod-dump --top output."""
    counts: dict[str, int] = {}
    total = 0
    block = re.compile(
        r"^\s+(\d+)\s+samples\s+([0-9.]+)%.*?$\n\s+(0x[0-9a-fA-F]+)",
        re.M,
    )
    for match in block.finditer(dump_text):
        samples = int(match.group(1))
        pc = int(match.group(3), 16)
        name = symbols.get(pc, f"{pc:#x}")
        counts[name] = counts.get(name, 0) + samples
        total += samples
    rows = []
    for name, samples in sorted(counts.items(), key=lambda kv: -kv[1]):
        rows.append(
            {
                "symbol": name,
                "self": samples,
                "self_percent": (100.0 * samples / total) if total else 0.0,
            }
        )
    return rows


def capture_orbit_file(orbit: Path, stockfish: Path, out_dir: Path) -> dict[str, Any]:
    """Fallback: orbit-service --pid <tid> --duration-ms --out (one thread)."""
    pod = out_dir / "capture.pod"
    target = start_uci(stockfish)
    log = out_dir / "orbit-file.log"
    # Search runs on a worker thread, not the UCI tid. File-mode --pid is a tid.
    # Do not send setoption here: ThreadPool::set reloads the net on the UCI
    # thread and races the 4s capture window.
    time.sleep(0.4)
    others: list[int] = []
    task_dir = Path(f"/proc/{target.pid}/task")
    if task_dir.is_dir():
        others = [int(p.name) for p in task_dir.iterdir() if p.name != str(target.pid)]
    # File-mode sampling of a non-leader tid returned 0 samples on this VM
    # even as root. The UCI thread group leader does produce samples.
    sample_tid = target.pid
    maps_text = ""
    maps_path = Path(f"/proc/{target.pid}/maps")
    if maps_path.exists():
        maps_text = maps_path.read_text()
        (out_dir / "maps.txt").write_text(maps_text)
    try:
        svc = subprocess.Popen(
            [
                str(orbit),
                "--pid",
                str(sample_tid),
                "--duration-ms",
                "6000",
                "--out",
                str(pod),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        time.sleep(0.4)
        uci_movetime(target, 4000)
        try:
            out, _ = svc.communicate(timeout=20)
        except subprocess.TimeoutExpired:
            svc.kill()
            out, _ = svc.communicate(timeout=5)
        log.write_text(out or "")
        dump_text = ""
        if ORBIT_POD_DUMP.is_file() and pod.exists():
            dump = run([str(ORBIT_POD_DUMP), str(pod), "--top", "20"], check=False)
            dump_text = dump.stdout + dump.stderr
            (out_dir / "orbit-pod-dump.txt").write_text(dump_text)
        samples = 0
        match = re.search(r"captured (\d+) samples", out or "")
        if match:
            samples = int(match.group(1))
        pcs = [int(x, 16) for x in re.findall(r"^\s+(0x[0-9a-fA-F]+)$", dump_text, re.M)]
        symbols = symbolize_pcs(stockfish, maps_text, pcs) if maps_text and pcs else {}
        hotspots = hotspots_from_pod_dump(dump_text, symbols)
        return {
            "backend": "orbit-file",
            "ok": samples > 0,
            "samples": samples,
            "command": f"{orbit} --pid {sample_tid} --duration-ms 6000 --out {pod}",
            "sample_tid": sample_tid,
            "uci_pid": target.pid,
            "worker_tids": others,
            "note": (
                "This Orbit file-mode capture (`orbit-service --pid --out`). "
                f"Sampled thread-group leader tid {sample_tid} (workers {others} "
                "returned 0 samples on this VM, including under sudo). "
                "Leaf PCs symbolized with /proc/pid/maps + addr2line. "
                "Serve-mode /api/sampling/report is tried first; it loaded "
                "symbols but recorded 0 callstack samples here."
            ),
            "pod": str(pod) if pod.exists() else None,
            "log": (out or "")[-800:],
            "dump_tail": dump_text[-800:],
            "hotspots": hotspots,
        }
    finally:
        if target.poll() is None:
            try:
                if target.stdin:
                    target.stdin.write("quit\n")
                    target.stdin.flush()
                target.wait(timeout=5)
            except Exception:  # noqa: BLE001
                target.kill()


def run_profile(mode: str, stockfish: Path, out_dir: Path, orbit: Path | None) -> dict[str, Any]:
    """Pluggable capture: orbit-http (preferred), then perf, then orbit-file."""
    out_dir.mkdir(parents=True, exist_ok=True)
    attempts: list[dict[str, Any]] = []

    def want(name: str) -> bool:
        return mode in ("auto", name, "all")

    if want("orbit") and orbit and orbit.is_file():
        cap = OrbitHttpCapture(orbit, out_dir)
        try:
            cap.start()
            result = cap.capture_bench(stockfish)
            attempts.append(result)
            if result.get("ok") and mode != "all":
                result = dict(result)
                result["attempts"] = list(attempts)
                return result
        except Exception as error:  # noqa: BLE001
            attempts.append(
                {
                    "backend": "orbit-http",
                    "ok": False,
                    "reason": str(error),
                    "service_log": str(out_dir / "orbit-service.log"),
                }
            )
        finally:
            cap.stop()
            # Let serve-mode release its perf rings before file-mode opens one.
            time.sleep(1.5)
    elif want("orbit") and (not orbit or not orbit.is_file()):
        attempts.append(
            {
                "backend": "orbit-http",
                "ok": False,
                "skipped": True,
                "reason": (
                    "orbit-service binary not found. Build with: "
                    "cargo +1.88.0 build --release --manifest-path "
                    "rust/crates/orbit-service/Cargo.toml"
                ),
                "expected": str(ORBIT_SERVICE),
            }
        )

    if want("perf"):
        perf_result = capture_perf(stockfish, out_dir)
        attempts.append(perf_result)
        if perf_result.get("ok") and mode != "all":
            perf_result = dict(perf_result)
            perf_result["attempts"] = list(attempts)
            return perf_result

    if want("orbit") and orbit and orbit.is_file() and mode in ("auto", "all", "orbit"):
        attempts.append(capture_orbit_file(orbit, stockfish, out_dir))

    chosen = next((dict(a) for a in attempts if a.get("ok")), None)
    if chosen is None:
        chosen = {
            "backend": "none",
            "ok": False,
            "reason": "no capture backend produced a profile",
        }
    # Shallow-copied so nesting attempts cannot create a JSON cycle.
    chosen["attempts"] = [{k: v for k, v in a.items() if k != "attempts"} for a in attempts]
    return chosen


# ---------------------------------------------------------------------------
# Compare + render
# ---------------------------------------------------------------------------


def compare_summaries(baseline: dict[str, Any], current: dict[str, Any]) -> dict[str, Any]:
    b_speed = (baseline.get("speedtest") or {}).get("nps")
    c_speed = (current.get("speedtest") or {}).get("nps")
    b_bench = ((baseline.get("bench") or {}).get("nps") or {}).get("mean")
    c_bench = ((current.get("bench") or {}).get("nps") or {}).get("mean")
    b_stdev = ((baseline.get("bench") or {}).get("nps") or {}).get("stdev")
    c_stdev = ((current.get("bench") or {}).get("nps") or {}).get("stdev")
    return {
        "baseline_path": baseline.get("_path"),
        "baseline_timestamp": baseline.get("timestamp"),
        "speedtest_nps": {
            "baseline": b_speed,
            "current": c_speed,
            "delta": None if b_speed is None or c_speed is None else c_speed - b_speed,
            "delta_percent": pct_delta(b_speed, c_speed),
        },
        "bench_nps_mean": {
            "baseline": b_bench,
            "current": c_bench,
            "delta": None if b_bench is None or c_bench is None else c_bench - b_bench,
            "delta_percent": pct_delta(b_bench, c_bench),
        },
        "bench_nps_stdev": {"baseline": b_stdev, "current": c_stdev},
        "perft_passed": {
            "baseline": (baseline.get("perft") or {}).get("passed"),
            "current": (current.get("perft") or {}).get("passed"),
        },
    }


def fmt_nps(value: float | None) -> str:
    if value is None:
        return "n/a"
    return f"{value:,.0f}"


def fmt_delta(entry: dict[str, Any] | None) -> str:
    if not entry or entry.get("delta_percent") is None:
        return "n/a"
    sign = "+" if entry["delta_percent"] >= 0 else ""
    return f"{sign}{entry['delta_percent']:.2f}% ({sign}{entry['delta']:,.0f})"


def render_markdown(summary: dict[str, Any]) -> str:
    bench = summary.get("bench") or {}
    nps = bench.get("nps") or {}
    times = bench.get("time_ms") or {}
    speed = summary.get("speedtest") or {}
    perft = summary.get("perft") or {}
    profile = summary.get("profile") or {}
    compare = summary.get("compare")
    lines = [
        "# Stockfish Orbit loop — suite summary",
        "",
        f"- Timestamp: `{summary.get('timestamp')}`",
        f"- Host: `{summary.get('hostname')}`",
        f"- Stockfish SHA: `{summary.get('stockfish_sha')}`",
        f"- Binary: `{summary.get('stockfish_binary')}`",
        "",
        "## Commands",
        "",
        "```",
        f"speedtest: {' '.join(map(str, speed.get('command') or [])) or speed.get('command')}",
        f"bench:     {' '.join(map(str, bench.get('command') or [])) or bench.get('command')}",
        f"perft:     {perft.get('command')}",
        f"profile:   {profile.get('command')}",
        "```",
        "",
        "## Speedtest (primary)",
        "",
    ]
    if speed:
        lines += [
            f"- Invocation: `{speed.get('filled_invocation') or speed.get('command')}`",
            f"- Threads: {speed.get('threads')}  Hash: {speed.get('hash_mib')} MiB  "
            f"Requested: {speed.get('requested_seconds')} s",
            f"- Nodes: {speed.get('nodes')}",
            f"- Time: {speed.get('time_s')} s",
            f"- **Nodes/second: {fmt_nps(speed.get('nps'))}**",
            "",
        ]
    else:
        lines += ["Skipped.", ""]
    lines += [
        "## Repeated bench (secondary)",
        "",
        f"`bench {' '.join(BENCH_ARGS)}` × {bench.get('iterations', 0)}",
        "",
        f"- Nodes searched (fingerprint): {bench.get('nodes_searched')}",
        f"- **nps mean ± stdev: {fmt_nps(nps.get('mean'))} ± {fmt_nps(nps.get('stdev'))}**",
        f"- nps min / max: {fmt_nps(nps.get('min'))} / {fmt_nps(nps.get('max'))}",
        f"- time ms mean ± stdev: {times.get('mean'):.1f} ± {times.get('stdev'):.1f}"
        if times.get("mean") is not None
        else "- time: n/a",
        "",
        "## Perft (correctness)",
        "",
        f"- Mode: `{perft.get('mode')}`",
        f"- Passed: **{perft.get('passed')}**",
    ]
    if perft.get("error"):
        lines.append(f"- Error: {perft['error']}")
    if perft.get("note"):
        lines.append(f"- Note: {perft['note']}")
    failed = [t for t in (perft.get("tests") or []) if not t.get("ok")]
    if failed:
        lines.append("- Failures:")
        for test in failed:
            lines.append(
                f"  - depth {test['depth']}: expected {test['expected']} got {test['got']} "
                f"({test['position'][:60]})"
            )
    lines += ["", "## Profile", "", f"- Backend: `{profile.get('backend')}`"]
    if profile.get("reason"):
        lines.append(f"- Reason: {profile['reason']}")
    if profile.get("samples") is not None:
        lines.append(f"- Samples: {profile['samples']}")
    if profile.get("note"):
        lines.append(f"- Note: {profile['note']}")
    hotspots = profile.get("hotspots") or []
    if hotspots:
        lines += [
            "",
            "| % self | symbol |",
            "| ---: | --- |",
        ]
        for row in hotspots[:15]:
            pct = row.get("self_percent")
            pct_s = f"{pct:.2f}" if isinstance(pct, (int, float)) else "—"
            lines.append(f"| {pct_s} | `{row.get('symbol', '??')}` |")
    else:
        lines.append("- No hotspot table (backend skipped or produced no symbols).")
    if compare:
        lines += [
            "",
            "## Before / after",
            "",
            f"- Baseline: `{compare.get('baseline_path')}` ({compare.get('baseline_timestamp')})",
            f"- Speedtest nps: {fmt_delta(compare.get('speedtest_nps'))}",
            f"- Bench mean nps: {fmt_delta(compare.get('bench_nps_mean'))}",
            f"- Perft: {compare.get('perft_passed')}",
        ]
    lines += [
        "",
        "## Compiler",
        "",
        "```",
        summary.get("compiler") or "",
        "```",
        "",
    ]
    return "\n".join(lines)


def print_compare(compare: dict[str, Any]) -> None:
    print("\nBefore / after")
    print(f"  baseline: {compare.get('baseline_path')} ({compare.get('baseline_timestamp')})")
    print(f"  speedtest nps: {fmt_delta(compare.get('speedtest_nps'))}")
    print(f"  bench mean nps: {fmt_delta(compare.get('bench_nps_mean'))}")
    print(f"  perft: {compare.get('perft_passed')}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def default_out_dir() -> Path:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return ROOT / "docs" / "stockfish-orbit-loop" / "runs" / stamp


def load_summary(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text())
    data["_path"] = str(path)
    return data


def find_orbit_service(explicit: str | None) -> Path | None:
    if explicit:
        path = Path(explicit).expanduser().resolve()
        return path if path.is_file() else None
    for candidate in (
        ORBIT_SERVICE,
        ROOT / "rust" / "crates" / "orbit-service" / "target" / "x86_64-unknown-linux-musl" / "release" / "orbit-service",
        Path(shutil.which("orbit-service") or ""),
    ):
        if candidate and candidate.is_file():
            return candidate
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stockfish", type=Path, default=STOCKFISH_BIN)
    parser.add_argument("--out", type=Path, default=None, help="directory for logs + summary (created)")
    parser.add_argument("--speedtest-threads", type=int, default=None, help="default: all CPUs")
    parser.add_argument(
        "--speedtest-hash",
        type=int,
        default=None,
        help=f"MiB (default: threads * {SPEEDTEST_HASH_PER_THREAD_MIB})",
    )
    parser.add_argument(
        "--speedtest-seconds",
        type=int,
        default=20,
        help=f"suite default 20; official speedtest default is {SPEEDTEST_OFFICIAL_SECONDS}",
    )
    parser.add_argument("--bench-iters", type=int, default=20)
    parser.add_argument("--perft", choices=("smoke", "official", "off"), default="smoke")
    parser.add_argument(
        "--capture",
        choices=("auto", "orbit", "perf", "all", "none"),
        default="auto",
        help="auto: Orbit HTTP if orbit-service exists, else perf, else skip",
    )
    parser.add_argument("--orbit-service", default=None, help="path to this repo's orbit-service")
    parser.add_argument("--baseline", type=Path, help="previous summary.json for deltas")
    parser.add_argument("--compare", nargs=2, metavar=("OLD", "NEW"), help="compare two summaries and exit")
    parser.add_argument("--skip-speedtest", action="store_true")
    parser.add_argument("--skip-bench", action="store_true")
    args = parser.parse_args(argv)

    if args.compare:
        old = load_summary(Path(args.compare[0]))
        new = load_summary(Path(args.compare[1]))
        cmp = compare_summaries(old, new)
        print(json.dumps(cmp, indent=2))
        print_compare(cmp)
        return 0

    binary = args.stockfish.resolve()
    ensure_stockfish(binary)
    out_dir = (args.out or default_out_dir()).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    threads = args.speedtest_threads or nproc()
    hash_mib = args.speedtest_hash or threads * SPEEDTEST_HASH_PER_THREAD_MIB
    orbit = find_orbit_service(args.orbit_service)

    summary: dict[str, Any] = {
        "timestamp": utc_now(),
        "hostname": socket.gethostname(),
        "stockfish_sha": git_sha(ROOT / "stockfish"),
        "stockfish_pinned_sha": PINNED_SHA,
        "stockfish_binary": str(binary),
        "compiler": stockfish_compiler(binary),
        "commands": {
            "speedtest_official": "./stockfish speedtest   # threads=nproc hash=threads*128 seconds=150",
            "speedtest_suite": f"./stockfish speedtest {threads} {hash_mib} {args.speedtest_seconds}",
            "bench": f"./stockfish bench {' '.join(BENCH_ARGS)}",
            "perft_official": f"cd stockfish/src && ../tests/perft.sh",
        },
    }

    print(f"suite output: {out_dir}", flush=True)
    print(f"stockfish:    {binary}  sha={summary['stockfish_sha']}", flush=True)
    if args.capture != "none":
        summary["perf_event_paranoid"] = relax_perf_paranoid(1)
        print(
            f"perf_event_paranoid: {summary['perf_event_paranoid'].get('before')} "
            f"-> {summary['perf_event_paranoid'].get('after')}",
            flush=True,
        )

    if args.skip_speedtest:
        summary["speedtest"] = None
    else:
        print(
            f"speedtest {threads} threads, {hash_mib} MiB, {args.speedtest_seconds}s "
            f"(official default is {SPEEDTEST_OFFICIAL_SECONDS}s)...",
            flush=True,
        )
        summary["speedtest"] = run_speedtest(
            binary, threads, hash_mib, args.speedtest_seconds, out_dir / "speedtest.log"
        )
        print(f"  speedtest nps: {summary['speedtest']['nps']}", flush=True)

    if args.skip_bench:
        summary["bench"] = None
    else:
        print(f"bench {' '.join(BENCH_ARGS)} × {args.bench_iters}...", flush=True)
        summary["bench"] = run_benches(binary, args.bench_iters, out_dir / "bench.log")
        nps = summary["bench"]["nps"]
        print(f"  bench nps mean±stdev: {nps['mean']:.0f} ± {nps['stdev']:.0f}", flush=True)

    if args.perft == "off":
        summary["perft"] = {"mode": "off", "passed": None, "command": None}
    elif args.perft == "official":
        summary["perft"] = run_perft_official(out_dir / "perft.log")
    else:
        summary["perft"] = run_perft_smoke(binary, out_dir / "perft.log")
    print(f"  perft {summary['perft'].get('mode')}: passed={summary['perft'].get('passed')}", flush=True)

    if args.capture == "none":
        summary["profile"] = {
            "backend": "none",
            "ok": False,
            "skipped": True,
            "reason": "capture disabled",
            "hotspots": [],
            "plug": (
                "Pass --capture orbit after building rust/crates/orbit-service, "
                "or --capture perf if the perf CLI is installed."
            ),
        }
    else:
        print(f"profile capture ({args.capture})...", flush=True)
        summary["profile"] = run_profile(args.capture, binary, out_dir / "capture", orbit)
        print(
            f"  profile backend={summary['profile'].get('backend')} "
            f"ok={summary['profile'].get('ok')}",
            flush=True,
        )

    if args.baseline:
        baseline = load_summary(args.baseline.resolve())
        summary["compare"] = compare_summaries(baseline, summary)
        print_compare(summary["compare"])

    json_path = out_dir / "summary.json"
    md_path = out_dir / "summary.md"
    json_path.write_text(json.dumps(summary, indent=2, default=str) + "\n")
    md_path.write_text(render_markdown(summary))
    print(f"\nwrote {json_path}\nwrote {md_path}")
    if summary.get("bench") and summary["bench"].get("nps"):
        nps = summary["bench"]["nps"]
        print(f"bench nps: {nps['mean']:.0f} ± {nps['stdev']:.0f}")
    return 0 if (summary.get("perft") or {}).get("passed") is not False else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
