#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Closed loop: baseline → hotspots → propose → apply → rebuild → gate → revert.

  python3 tools/optimize-loop/loop.py --mode mock
  python3 tools/optimize-loop/loop.py --mode live --config docs/optimize-loop/examples/fixture.toml
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import ops  # noqa: E402
from config import ROOT  # noqa: E402

LOG_DIR = ROOT / "docs" / "optimize-loop" / "loop-log"
DEFAULT_MIN_GAIN_PERCENT = 0.5
DEFAULT_SIGMA = 1.0

LIBC_OR_RT = ("__", "malloc", "free", "mmap", "madvise", "munmap", "nss_", "libc", "ld-linux", "pthread")


def _now() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def classify_hotspots(hotspots: list[dict[str, Any]]) -> dict[str, Any]:
    libc_like: list[str] = []
    other: list[str] = []
    for row in hotspots:
        symbol = str(row.get("symbol") or "")
        low = symbol.lower()
        if any(h.lower() in low for h in LIBC_OR_RT) or symbol in ("(see dump)",):
            libc_like.append(symbol)
        elif symbol:
            other.append(symbol)
    if other and not libc_like:
        quality = "usable"
        reason = "non-libc symbols present; still do not auto-author an optimization"
    elif not hotspots:
        quality = "empty"
        reason = "no hotspot rows; capture skipped or produced nothing"
    else:
        quality = "poor"
        reason = (
            "top symbols look like libc/runtime or are unsymbolized. "
            "A labeled no-op experiment is the honest path until capture shows the real hot path."
        )
    return {"quality": quality, "reason": reason, "libc_like": libc_like, "other": other, "count": len(hotspots)}


def evaluate_gate(
    *,
    baseline_mean: float | None,
    current_mean: float | None,
    baseline_stdev: float | None,
    min_gain_percent: float = DEFAULT_MIN_GAIN_PERCENT,
    sigma: float = DEFAULT_SIGMA,
    baseline_fingerprint: str | None = None,
    current_fingerprint: str | None = None,
    correctness_passed: bool | None = None,
) -> dict[str, Any]:
    reasons: list[str] = []
    fingerprint_ok = True
    if (
        baseline_fingerprint is not None
        and current_fingerprint is not None
        and baseline_fingerprint != current_fingerprint
    ):
        fingerprint_ok = False
        reasons.append(f"fingerprint changed {baseline_fingerprint} → {current_fingerprint}")
    if correctness_passed is False:
        fingerprint_ok = False
        reasons.append("correctness command failed")

    delta = None if baseline_mean is None or current_mean is None else current_mean - baseline_mean
    delta_percent = None
    if baseline_mean not in (None, 0) and current_mean is not None:
        delta_percent = 100.0 * (current_mean - baseline_mean) / baseline_mean
    sigma_threshold = None if baseline_stdev is None else sigma * baseline_stdev
    gain_ok = delta_percent is not None and delta_percent > min_gain_percent
    sigma_ok = True
    if sigma_threshold is not None and delta is not None:
        sigma_ok = delta > sigma_threshold
    if delta_percent is None:
        reasons.append("missing bench mean (cannot compare)")
    elif not gain_ok:
        reasons.append(f"mean gain {delta_percent:.3f}% is not > {min_gain_percent}%")
    if sigma_threshold is not None and delta is not None and not sigma_ok:
        reasons.append(
            f"delta {delta:,.4g} is not outside {sigma:g}σ (threshold {sigma_threshold:,.4g} from baseline stdev)"
        )
    if gain_ok and sigma_ok and fingerprint_ok and delta_percent is not None:
        extra = f"and {delta:,.4g} > {sigma:g}σ" if sigma_threshold is not None else "(no stdev; percent gate only)"
        reasons.append(f"gain {delta_percent:.3f}% > {min_gain_percent}% {extra}")
    accepted = bool(fingerprint_ok and gain_ok and sigma_ok and delta_percent is not None)
    return {
        "accepted": accepted,
        "decision": "accept" if accepted else "reject",
        "min_gain_percent": min_gain_percent,
        "sigma": sigma,
        "baseline_mean": baseline_mean,
        "current_mean": current_mean,
        "baseline_stdev": baseline_stdev,
        "delta": delta,
        "delta_percent": delta_percent,
        "sigma_threshold": sigma_threshold,
        "fingerprint_ok": fingerprint_ok,
        "reasons": reasons,
    }


def propose_noop(config: str | None, iteration_id: str) -> dict[str, Any]:
    target = ops.resolve_target(config)
    rel = "workload.py" if (target.project / "workload.py").is_file() else None
    if rel is None:
        # First regular file under project that looks like source.
        for cand in ("README.md", "main.c", "main.cpp", "src/main.c"):
            if (target.project / cand).is_file():
                rel = cand
                break
    if rel is None:
        return {"ok": False, "error": "no obvious source file for a no-op experiment"}
    dest = target.project / rel
    old = dest.read_text()
    marker = f"# optimize-loop {iteration_id}: intentional no-op. Gate should reject if capture is poor.\n"
    if marker.split("\n", 1)[0] in old:
        return {"ok": False, "error": "no-op marker already present; revert first"}
    new = marker + old
    preview = ops.apply_patch(config=config, path=rel, content=new, apply=False)
    preview["kind"] = "noop_experiment"
    preview["target_path"] = rel
    preview["new_content"] = new
    return preview


def propose_mock() -> dict[str, Any]:
    return {
        "ok": True,
        "kind": "mock",
        "applied": False,
        "dry_run": True,
        "target_path": "workload.py",
        "diff": "--- a/workload.py\n+++ b/workload.py\n@@ -0,0 +1 @@\n+# optimize-loop mock: not applied\n",
        "note": "Synthetic proposal for gate/CI. The target tree is not touched.",
    }


def _next_log_id(log_dir: Path) -> str:
    existing = []
    if log_dir.is_dir():
        for child in log_dir.iterdir():
            if child.is_dir() and child.name[:4].isdigit():
                existing.append(int(child.name[:4]))
    return f"{(max(existing) + 1) if existing else 1:04d}"


def _write_attempt(log_dir: Path, slug: str, attempt: dict[str, Any]) -> Path:
    dest = log_dir / slug
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "attempt.json").write_text(json.dumps(attempt, indent=2, default=str) + "\n")
    if attempt.get("diff"):
        (dest / "proposal.diff").write_text(str(attempt["diff"]))
    gate = attempt.get("gate") or {}
    md = [
        f"# Loop attempt {attempt.get('id')} — {str(attempt.get('decision', '')).upper()}",
        "",
        f"- mode: `{attempt.get('mode')}`",
        f"- proposal: `{attempt.get('proposal_kind')}`",
        f"- decision: **{attempt.get('decision')}**",
        f"- reverted: {attempt.get('reverted')}",
        "",
    ]
    if gate.get("delta_percent") is not None:
        md.append(
            f"- metric: {gate.get('baseline_mean')} → {gate.get('current_mean')} "
            f"({gate.get('delta_percent'):+.3f}%)"
        )
    for reason in gate.get("reasons") or []:
        md.append(f"- {reason}")
    if attempt.get("hotspot_reason"):
        md.append(f"- {attempt['hotspot_reason']}")
    md.append("")
    (dest / "decision.md").write_text("\n".join(md) + "\n")
    return dest


def _rewrite_index(log_dir: Path) -> None:
    rows = []
    for child in sorted(log_dir.iterdir()):
        attempt_path = child / "attempt.json"
        if not attempt_path.is_file():
            continue
        data = json.loads(attempt_path.read_text())
        rows.append(
            {
                "id": data.get("id"),
                "slug": child.name,
                "mode": data.get("mode"),
                "decision": data.get("decision"),
                "proposal_kind": data.get("proposal_kind"),
                "delta_percent": (data.get("gate") or {}).get("delta_percent"),
                "timestamp": data.get("timestamp"),
            }
        )
    (log_dir / "index.json").write_text(json.dumps({"attempts": rows}, indent=2) + "\n")
    lines = [
        "# Optimize-loop attempt log",
        "",
        "| id | mode | proposal | decision | Δ % | when |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for row in rows:
        pct = row.get("delta_percent")
        pct_s = "n/a" if pct is None else f"{pct:+.3f}%"
        lines.append(
            f"| {row.get('id')} | {row.get('mode')} | {row.get('proposal_kind')} | "
            f"{row.get('decision')} | {pct_s} | {row.get('timestamp')} |"
        )
    lines.append("")
    (log_dir / "index.md").write_text("\n".join(lines))


def run_mock(log_dir: Path, min_gain_percent: float, sigma: float) -> dict[str, Any]:
    fixtures = [
        {"slug": "mock-reject-below-floor", "b": 1_000_000.0, "c": 1_002_000.0, "s": 15_000.0, "expect": "reject"},
        {"slug": "mock-reject-inside-sigma", "b": 1_000_000.0, "c": 1_006_000.0, "s": 15_000.0, "expect": "reject"},
        {"slug": "mock-accept-above-sigma", "b": 1_000_000.0, "c": 1_020_000.0, "s": 15_000.0, "expect": "accept"},
        {
            "slug": "mock-reject-fingerprint",
            "b": 1_000_000.0,
            "c": 1_050_000.0,
            "s": 1_000.0,
            "bf": "aaa",
            "cf": "bbb",
            "expect": "reject",
        },
    ]
    results = []
    failed = False
    for fixture in fixtures:
        gate = evaluate_gate(
            baseline_mean=fixture["b"],
            current_mean=fixture["c"],
            baseline_stdev=fixture["s"],
            min_gain_percent=min_gain_percent,
            sigma=sigma,
            baseline_fingerprint=fixture.get("bf"),
            current_fingerprint=fixture.get("cf"),
        )
        ok = gate["decision"] == fixture["expect"]
        failed = failed or (not ok)
        ident = _next_log_id(log_dir)
        attempt = {
            "id": ident,
            "mode": "mock",
            "timestamp": _now(),
            "proposal_kind": "mock",
            "diff": propose_mock()["diff"],
            "gate": gate,
            "decision": gate["decision"],
            "reverted": True,
            "applied": False,
            "expect": fixture["expect"],
            "gate_selftest_ok": ok,
            "hotspot_quality": "poor",
        }
        dest = _write_attempt(log_dir, f"{ident}-{fixture['slug']}", attempt)
        results.append({"dir": str(dest), "ok": ok, "gate": gate, "expect": fixture["expect"]})
    _rewrite_index(log_dir)
    return {"ok": not failed, "mode": "mock", "attempts": results, "log_dir": str(log_dir)}


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    log_dir = Path(args.log_dir)
    log_dir.mkdir(parents=True, exist_ok=True)
    ident = _next_log_id(log_dir)
    slug = f"{ident}-live-{args.proposal}"
    attempt: dict[str, Any] = {
        "id": ident,
        "mode": "live",
        "timestamp": _now(),
        "proposal_kind": "noop_experiment",
        "applied": False,
        "reverted": False,
        "decision": "reject",
        "config": args.config,
    }
    spots = ops.inspect_hotspots(limit=15, summary_path=args.hotspots_from)
    classification = classify_hotspots(list(spots.get("hotspots") or []))
    attempt["hotspots"] = spots.get("hotspots") or []
    attempt["hotspot_quality"] = classification["quality"]
    attempt["hotspot_reason"] = classification["reason"]
    attempt["hotspot_classification"] = classification

    proposal = propose_noop(args.config, ident)
    if not proposal.get("ok"):
        attempt["error"] = proposal.get("error")
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}
    attempt["diff"] = proposal.get("diff")
    attempt["target_path"] = proposal.get("target_path")

    if args.baseline:
        baseline = ops.load_summary(args.baseline)
    else:
        ran = ops.run_suite(
            config=args.config,
            out=args.baseline_out,
            bench_iters=args.bench_iters,
            capture=args.capture,
            skip_build=False,
        )
        attempt["baseline_suite"] = {"ok": ran.get("ok"), "summary_path": ran.get("summary_path"), "bench": ran.get("bench")}
        baseline = ops.load_summary(ran.get("summary_path"))
    if not baseline.get("ok"):
        attempt["error"] = baseline.get("error") or "baseline failed"
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}
    attempt["baseline"] = baseline.get("bench")

    applied = ops.apply_patch(
        config=args.config,
        path=proposal["target_path"],
        content=proposal["new_content"],
        apply=True,
    )
    attempt["applied"] = bool(applied.get("applied"))
    if not applied.get("ok"):
        attempt["error"] = applied.get("error")
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}

    rebuild = ops.rebuild(args.config)
    attempt["rebuild"] = {"ok": rebuild.get("ok"), "skipped": rebuild.get("skipped")}
    if not rebuild.get("ok"):
        ops.revert_project(args.config, [proposal["target_path"]])
        attempt["reverted"] = True
        attempt["error"] = "rebuild failed"
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}

    after_run = ops.run_suite(
        config=args.config,
        out=args.after_out,
        baseline=str(baseline.get("_path")),
        bench_iters=args.bench_iters,
        capture="none",
        skip_build=True,
    )
    after = ops.load_summary(after_run.get("summary_path"))
    attempt["after"] = after.get("bench")
    attempt["after_suite"] = {"ok": after_run.get("ok"), "summary_path": after_run.get("summary_path"), "compare": after_run.get("compare")}

    b, a = attempt["baseline"] or {}, attempt["after"] or {}
    gate = evaluate_gate(
        baseline_mean=b.get("mean"),
        current_mean=a.get("mean"),
        baseline_stdev=b.get("stdev"),
        min_gain_percent=args.min_gain_percent,
        sigma=args.sigma,
        baseline_fingerprint=b.get("fingerprint"),
        current_fingerprint=a.get("fingerprint"),
        correctness_passed=(after.get("correctness") or {}).get("passed"),
    )
    attempt["gate"] = gate
    attempt["decision"] = gate["decision"]
    if gate["accepted"]:
        attempt["reverted"] = False
        attempt["kept"] = "Accepted. Change stays in the target working tree — not committed or pushed."
    else:
        revert = ops.revert_project(args.config, [proposal["target_path"]])
        attempt["reverted"] = True
        attempt["revert"] = revert
        ops.rebuild(args.config)
    dest = _write_attempt(log_dir, slug, attempt)
    _rewrite_index(log_dir)
    attempt["ok"] = True
    attempt["log_dir"] = str(dest)
    return attempt


def run_loop(**kwargs: Any) -> dict[str, Any]:
    parser = build_parser()
    ns = parser.parse_args([])
    for key, value in kwargs.items():
        if value is not None and hasattr(ns, key):
            setattr(ns, key, value)
    ns.log_dir = Path(ns.log_dir)
    ns.log_dir.mkdir(parents=True, exist_ok=True)
    if ns.mode == "mock":
        return run_mock(ns.log_dir, ns.min_gain_percent, ns.sigma)
    return run_live(ns)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--mode", choices=("mock", "live"), default="mock")
    parser.add_argument("--proposal", choices=("auto", "noop"), default="auto")
    parser.add_argument("--config", default=str(ops.cfgmod.DEFAULT_CONFIG))
    parser.add_argument("--log-dir", default=str(LOG_DIR))
    parser.add_argument("--baseline")
    parser.add_argument("--baseline-out")
    parser.add_argument("--after-out")
    parser.add_argument("--hotspots-from")
    parser.add_argument("--bench-iters", type=int, default=8)
    parser.add_argument("--capture", choices=("auto", "orbit", "perf", "none"), default="none")
    parser.add_argument("--min-gain-percent", type=float, default=DEFAULT_MIN_GAIN_PERCENT)
    parser.add_argument("--sigma", type=float, default=DEFAULT_SIGMA)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    args.log_dir = Path(args.log_dir)
    args.log_dir.mkdir(parents=True, exist_ok=True)
    result = run_mock(args.log_dir, args.min_gain_percent, args.sigma) if args.mode == "mock" else run_live(args)
    print(json.dumps(result, indent=2, default=str))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
