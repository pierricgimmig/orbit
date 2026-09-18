#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Run a configured external bench/correctness/capture suite.

  python3 tools/optimize-loop/suite.py --config docs/optimize-loop/examples/fixture.toml
  python3 tools/optimize-loop/suite.py --compare old.json new.json
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import capture  # noqa: E402
import config as cfgmod  # noqa: E402
from config import ROOT, TargetConfig  # noqa: E402
from metrics import parse_metric  # noqa: E402

RUNS = ROOT / "docs" / "optimize-loop" / "runs"


def _run(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float,
) -> subprocess.CompletedProcess[str]:
    merged = os.environ.copy()
    merged.update(env)
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=merged,
        timeout=timeout,
        text=True,
        capture_output=True,
        check=False,
    )


def _mean_stdev(values: list[float]) -> tuple[float, float]:
    if not values:
        return (0.0, 0.0)
    if len(values) == 1:
        return (values[0], 0.0)
    return (statistics.mean(values), statistics.stdev(values))


def pct_delta(old: float | None, new: float | None) -> float | None:
    if old is None or new is None or old == 0:
        return None
    return 100.0 * (new - old) / old


def run_build(target: TargetConfig) -> dict[str, Any]:
    if not target.build_cmd:
        return {"ok": True, "skipped": True, "command": [], "note": "no build.command"}
    proc = _run(target.build_cmd, cwd=target.build_workdir, env=target.env, timeout=target.build_timeout)
    return {
        "ok": proc.returncode == 0,
        "command": target.build_cmd,
        "returncode": proc.returncode,
        "stdout_tail": (proc.stdout or "")[-1500:],
        "stderr_tail": (proc.stderr or "")[-800:],
    }


def run_bench(target: TargetConfig, iters: int | None = None) -> dict[str, Any]:
    n = iters if iters is not None else target.bench_repeat
    values: list[float] = []
    fingerprints: list[str] = []
    logs: list[dict[str, Any]] = []
    for i in range(max(1, n)):
        proc = _run(target.bench_cmd, cwd=target.bench_workdir, env=target.env, timeout=target.bench_timeout)
        parsed = parse_metric(proc.stdout + "\n" + proc.stderr, target.bench_metric)
        logs.append(
            {
                "iteration": i + 1,
                "returncode": proc.returncode,
                "value": parsed.get("value"),
                "fingerprint": parsed.get("fingerprint"),
            }
        )
        if parsed.get("value") is not None:
            values.append(float(parsed["value"]))
        if parsed.get("fingerprint") is not None:
            fingerprints.append(str(parsed["fingerprint"]))
    mean, stdev = _mean_stdev(values)
    fp = fingerprints[0] if fingerprints and all(x == fingerprints[0] for x in fingerprints) else None
    return {
        "ok": bool(values) and all(row["returncode"] == 0 for row in logs),
        "command": target.bench_cmd,
        "iterations": n,
        "metric": target.bench_metric.get("name") or "throughput",
        "unit": target.bench_metric.get("unit") or "",
        "values": values,
        "mean": mean,
        "stdev": stdev,
        "min": min(values) if values else None,
        "max": max(values) if values else None,
        "fingerprint": fp,
        "fingerprints": fingerprints,
        "runs": logs,
    }


def run_correctness(target: TargetConfig) -> dict[str, Any]:
    if not target.correctness_cmd:
        return {"ok": True, "skipped": True, "passed": None, "command": []}
    proc = _run(
        target.correctness_cmd,
        cwd=target.correctness_workdir,
        env=target.env,
        timeout=target.correctness_timeout,
    )
    parsed = parse_metric(proc.stdout + "\n" + proc.stderr, {})
    passed = proc.returncode == target.correctness_expect
    return {
        "ok": passed,
        "passed": passed,
        "command": target.correctness_cmd,
        "returncode": proc.returncode,
        "expect_returncode": target.correctness_expect,
        "fingerprint": parsed.get("fingerprint"),
        "stdout_tail": (proc.stdout or "")[-800:],
    }


def run_secondary(target: TargetConfig) -> dict[str, Any] | None:
    if not target.secondary_cmd:
        return None
    proc = _run(
        target.secondary_cmd,
        cwd=target.secondary_workdir,
        env=target.env,
        timeout=target.secondary_timeout,
    )
    parsed = parse_metric(proc.stdout + "\n" + proc.stderr, target.secondary_metric)
    return {
        "ok": proc.returncode == 0 and parsed.get("ok"),
        "command": target.secondary_cmd,
        "metric": (target.secondary_metric or {}).get("name"),
        "value": parsed.get("value"),
        "fingerprint": parsed.get("fingerprint"),
        "returncode": proc.returncode,
    }


def compare_summaries(baseline: dict[str, Any], current: dict[str, Any]) -> dict[str, Any]:
    b = baseline.get("bench") or {}
    c = current.get("bench") or {}
    b_mean, c_mean = b.get("mean"), c.get("mean")
    return {
        "baseline_path": baseline.get("_path"),
        "metric": b.get("metric") or c.get("metric") or "throughput",
        "bench_mean": {
            "baseline": b_mean,
            "current": c_mean,
            "delta": None if b_mean is None or c_mean is None else c_mean - b_mean,
            "delta_percent": pct_delta(b_mean, c_mean),
        },
        "bench_stdev": {"baseline": b.get("stdev"), "current": c.get("stdev")},
        "fingerprint": {"baseline": b.get("fingerprint"), "current": c.get("fingerprint")},
        "correctness_passed": {
            "baseline": (baseline.get("correctness") or {}).get("passed"),
            "current": (current.get("correctness") or {}).get("passed"),
        },
        "secondary": {
            "baseline": ((baseline.get("secondary") or {}) or {}).get("value"),
            "current": ((current.get("secondary") or {}) or {}).get("value"),
            "delta_percent": pct_delta(
                ((baseline.get("secondary") or {}) or {}).get("value"),
                ((current.get("secondary") or {}) or {}).get("value"),
            ),
        },
    }


def render_markdown(summary: dict[str, Any]) -> str:
    bench = summary.get("bench") or {}
    lines = [
        f"# Optimize-loop suite — {summary.get('target_name', 'target')}",
        "",
        f"- time: {summary.get('timestamp')}",
        f"- project: `{summary.get('project')}`",
        f"- config: `{summary.get('config_path')}`",
        f"- metric: **{bench.get('metric')}** mean {bench.get('mean')} ± {bench.get('stdev')} ({bench.get('iterations')}×)",
        f"- fingerprint: {bench.get('fingerprint')}",
        f"- correctness: {(summary.get('correctness') or {}).get('passed')}",
        f"- profile backend: {(summary.get('profile') or {}).get('backend')}",
        "",
    ]
    compare = summary.get("compare")
    if compare:
        delta = (compare.get("bench_mean") or {}).get("delta_percent")
        lines.append(f"- vs baseline: {delta:+.3f}%" if delta is not None else "- vs baseline: n/a")
        lines.append("")
    hotspots = (summary.get("profile") or {}).get("hotspots") or []
    if hotspots:
        lines += ["| % self | symbol |", "| ---: | --- |"]
        for row in hotspots[:15]:
            pct = row.get("self_percent")
            pct_s = f"{pct:.2f}" if isinstance(pct, (int, float)) else "?"
            lines.append(f"| {pct_s} | `{row.get('symbol', '??')}` |")
        lines.append("")
    return "\n".join(lines) + "\n"


def _update_latest_pointer(out_dir: Path) -> None:
    runs = RUNS
    runs.mkdir(parents=True, exist_ok=True)
    latest = runs / "latest"
    target = out_dir.resolve()
    try:
        if latest.is_symlink() or latest.exists():
            latest.unlink()
        latest.symlink_to(target, target_is_directory=True)
    except OSError:
        (runs / "latest.path").write_text(str(target) + "\n")


def default_out_dir() -> Path:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return RUNS / stamp


def run_suite(
    target: TargetConfig,
    *,
    out: Path | None = None,
    baseline: Path | None = None,
    bench_iters: int | None = None,
    capture_mode: str = "auto",
    skip_bench: bool = False,
    skip_correctness: bool = False,
    skip_build: bool = False,
    skip_secondary: bool = False,
) -> dict[str, Any]:
    out_dir = (out or default_out_dir()).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    summary: dict[str, Any] = {
        "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "target_name": target.name,
        "project": str(target.project),
        "config_path": str(target.config_path),
        "host": os.uname().nodename if hasattr(os, "uname") else "",
    }
    if not skip_build:
        print("  build...", file=sys.stderr, flush=True)
        summary["build"] = run_build(target)
    if not skip_secondary:
        print("  secondary...", file=sys.stderr, flush=True)
        summary["secondary"] = run_secondary(target)
    if not skip_bench:
        print(f"  bench ×{bench_iters or target.bench_repeat}...", file=sys.stderr, flush=True)
        summary["bench"] = run_bench(target, bench_iters)
        print(
            f"    mean={summary['bench'].get('mean')} ± {summary['bench'].get('stdev')}",
            file=sys.stderr,
            flush=True,
        )
    if not skip_correctness:
        print("  correctness...", file=sys.stderr, flush=True)
        summary["correctness"] = run_correctness(target)
    if capture_mode != "none":
        print(f"  capture {capture_mode}...", file=sys.stderr, flush=True)
        cap_dir = out_dir / "capture"
        summary["profile"] = capture.run_profile(
            capture_mode,
            target.capture_cmd,
            cwd=target.capture_workdir,
            env=target.env,
            out_dir=cap_dir,
            duration_ms=target.capture_duration_ms,
        )
    else:
        summary["profile"] = {"backend": "none", "ok": False, "skipped": True, "hotspots": []}

    if baseline and baseline.is_file():
        old = json.loads(baseline.read_text())
        old["_path"] = str(baseline)
        summary["compare"] = compare_summaries(old, summary)

    json_path = out_dir / "summary.json"
    md_path = out_dir / "summary.md"
    json_path.write_text(json.dumps(summary, indent=2, default=str) + "\n")
    md_path.write_text(render_markdown(summary))
    _update_latest_pointer(out_dir)
    summary["_path"] = str(json_path)
    print(f"\nwrote {json_path}\nwrote {md_path}", file=sys.stderr)
    return summary


def load_summary_file(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text())
    data["_path"] = str(path)
    return data


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--config", default=str(cfgmod.DEFAULT_CONFIG))
    parser.add_argument("--out", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--compare", nargs=2, metavar=("OLD", "NEW"))
    parser.add_argument("--bench-iters", type=int)
    parser.add_argument("--capture", choices=("auto", "orbit", "perf", "all", "none"), default="auto")
    parser.add_argument("--skip-bench", action="store_true")
    parser.add_argument("--skip-correctness", action="store_true")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--skip-secondary", action="store_true")
    args = parser.parse_args(argv)
    if args.compare:
        old = load_summary_file(Path(args.compare[0]))
        new = load_summary_file(Path(args.compare[1]))
        print(json.dumps(compare_summaries(old, new), indent=2))
        return 0
    target = cfgmod.load_config(args.config)
    summary = run_suite(
        target,
        out=args.out,
        baseline=args.baseline,
        bench_iters=args.bench_iters,
        capture_mode=args.capture,
        skip_bench=args.skip_bench,
        skip_correctness=args.skip_correctness,
        skip_build=args.skip_build,
        skip_secondary=args.skip_secondary,
    )
    ok = True
    if summary.get("bench") and not summary["bench"].get("ok"):
        ok = False
    if (summary.get("correctness") or {}).get("passed") is False:
        ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
