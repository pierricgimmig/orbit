#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Phase 4 closed loop: baseline → hotspots → propose → apply → rebuild → compare → gate.

Uses the Phase 3 ops (same APIs as the MCP tools). Does not call an LLM and
does not push to git. Default live proposal is a labeled no-op when Orbit
hotspots are libc/startup rather than search — that is intentional honesty,
not a fake optimization win.

  python3 tools/stockfish-orbit-loop/loop.py --mode mock
  python3 tools/stockfish-orbit-loop/loop.py --mode live --proposal auto
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "mcp"))
import ops  # noqa: E402

LOG_DIR = ops.ROOT / "docs" / "stockfish-orbit-loop" / "loop-log"

# Gate defaults from the Phase 4 brief.
DEFAULT_MIN_GAIN_PERCENT = 0.5
DEFAULT_SIGMA = 1.0

LIBC_OR_RT = (
    "__",
    "malloc",
    "free",
    "mmap",
    "madvise",
    "munmap",
    "nss_",
    "libc",
    "ld-linux",
    "pthread",
)
STARTUP_HINTS = (
    "Network::load",
    "permute",
    "hash_bytes",
    "ThreadPool::set",
    "read_leb",
    "std::basic_streambuf",
    "xsgetn",
)
SEARCH_HINTS = (
    "Stockfish::Search",
    "qsearch",
    "MovePicker",
    "generate<",
    "do_move",
    "undo_move",
    "Position::see",
    "evaluate(",
    "Stockfish::Eval::evaluate",
    "nnue_evaluate",
)


def _now() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def classify_hotspots(hotspots: list[dict[str, Any]]) -> dict[str, Any]:
    search_like: list[str] = []
    libc_like: list[str] = []
    startup_like: list[str] = []
    other: list[str] = []
    for row in hotspots:
        symbol = str(row.get("symbol") or "")
        low = symbol.lower()
        # Libc/startup first: demangled signatures often embed Stockfish::Search::
        # types (e.g. ThreadPool::set(..., Search::SharedState)) without being
        # a search hot path.
        if any(h.lower() in low for h in LIBC_OR_RT):
            libc_like.append(symbol)
        elif any(h in symbol for h in STARTUP_HINTS):
            startup_like.append(symbol)
        elif any(h in symbol for h in SEARCH_HINTS):
            search_like.append(symbol)
        elif symbol:
            other.append(symbol)
    if search_like:
        quality = "usable"
        reason = "search-like symbols present; still do not auto-author an optimization"
    elif not hotspots:
        quality = "empty"
        reason = "no hotspot rows; capture skipped or produced nothing"
    else:
        quality = "poor"
        reason = (
            "top symbols look like libc/startup, not search. "
            "File-mode on this VM samples the UCI leader during attach, often "
            "before workers are in qsearch. A no-op experiment is the honest path."
        )
    return {
        "quality": quality,
        "reason": reason,
        "search_like": search_like,
        "libc_like": libc_like,
        "startup_like": startup_like,
        "other": other,
        "count": len(hotspots),
    }


def evaluate_gate(
    *,
    baseline_mean: float | None,
    current_mean: float | None,
    baseline_stdev: float | None,
    min_gain_percent: float = DEFAULT_MIN_GAIN_PERCENT,
    sigma: float = DEFAULT_SIGMA,
    baseline_nodes: int | None = None,
    current_nodes: int | None = None,
    perft_passed: bool | None = None,
) -> dict[str, Any]:
    """Accept only a statistically meaningful nps gain; never on a fingerprint break."""
    reasons: list[str] = []
    fingerprint_ok = True
    if baseline_nodes is not None and current_nodes is not None and baseline_nodes != current_nodes:
        fingerprint_ok = False
        reasons.append(f"bench fingerprint changed {baseline_nodes} → {current_nodes}")
    if perft_passed is False:
        fingerprint_ok = False
        reasons.append("perft failed")

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
        reasons.append("missing bench nps mean (cannot compare)")
    elif not gain_ok:
        reasons.append(f"mean nps gain {delta_percent:.3f}% is not > {min_gain_percent}%")
    if sigma_threshold is not None and delta is not None and not sigma_ok:
        reasons.append(
            f"delta {delta:,.0f} nps is not outside {sigma:g}σ "
            f"(threshold {sigma_threshold:,.0f} nps from baseline stdev)"
        )
    if gain_ok and sigma_ok and fingerprint_ok and delta_percent is not None:
        reasons.append(
            f"gain {delta_percent:.3f}% > {min_gain_percent}% "
            + (
                f"and {delta:,.0f} nps > {sigma:g}σ"
                if sigma_threshold is not None
                else "(no stdev; percent gate only)"
            )
        )

    accepted = bool(fingerprint_ok and gain_ok and sigma_ok and delta_percent is not None)
    return {
        "accepted": accepted,
        "decision": "accept" if accepted else "reject",
        "min_gain_percent": min_gain_percent,
        "sigma": sigma,
        "baseline_mean": baseline_mean,
        "current_mean": current_mean,
        "baseline_stdev": baseline_stdev,
        "delta_nps": delta,
        "delta_percent": delta_percent,
        "sigma_threshold_nps": sigma_threshold,
        "fingerprint_ok": fingerprint_ok,
        "reasons": reasons,
    }


def propose_noop(iteration_id: str) -> dict[str, Any]:
    path = "src/misc.cpp"
    dest = ops._safe_stockfish_path(path)
    old = dest.read_text() if dest.is_file() else ""
    needle = '#include "misc.h"\n'
    marker = (
        f"// stockfish-orbit-loop {iteration_id}: intentional no-op experiment.\n"
        "// Capture quality was libc/startup, not a search hotspot. Gate should reject.\n"
    )
    if needle not in old:
        return {"ok": False, "error": f"{path} missing expected include line"}
    if marker.split("\n", 1)[0] in old:
        return {"ok": False, "error": "no-op marker already present; revert first"}
    new = old.replace(needle, marker + needle, 1)
    preview = ops.apply_stockfish_patch(path=path, content=new, apply=False)
    preview["kind"] = "noop_experiment"
    preview["target_path"] = path
    preview["new_content"] = new
    return preview


def propose_mock() -> dict[str, Any]:
    diff = (
        "--- a/src/misc.cpp\n"
        "+++ b/src/misc.cpp\n"
        "@@ -0,0 +1,2 @@\n"
        "+// stockfish-orbit-loop mock: not applied\n"
        "+\n"
    )
    return {
        "ok": True,
        "kind": "mock",
        "applied": False,
        "dry_run": True,
        "target_path": "src/misc.cpp",
        "diff": diff,
        "note": "Synthetic proposal for gate/CI. stockfish/ is not touched.",
    }


def _bench_stats(summary: dict[str, Any]) -> dict[str, Any]:
    bench = summary.get("bench") or {}
    nps = bench.get("nps") or {}
    return {
        "path": summary.get("_path"),
        "mean": nps.get("mean"),
        "stdev": nps.get("stdev"),
        "nodes": bench.get("nodes_searched"),
        "iterations": bench.get("iterations"),
    }


def _next_log_id(log_dir: Path) -> str:
    existing = []
    if log_dir.is_dir():
        for child in log_dir.iterdir():
            if child.is_dir() and child.name[:4].isdigit():
                existing.append(int(child.name[:4]))
    nxt = (max(existing) + 1) if existing else 1
    return f"{nxt:04d}"


def _write_attempt(log_dir: Path, slug: str, attempt: dict[str, Any]) -> Path:
    dest = log_dir / slug
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "attempt.json").write_text(json.dumps(attempt, indent=2, default=str) + "\n")
    if attempt.get("diff"):
        (dest / "proposal.diff").write_text(str(attempt["diff"]))
    md = [
        f"# Loop attempt {attempt.get('id')} — {attempt.get('decision', '').upper()}",
        "",
        f"- mode: `{attempt.get('mode')}`",
        f"- proposal: `{attempt.get('proposal_kind')}`",
        f"- decision: **{attempt.get('decision')}**",
        f"- reverted: {attempt.get('reverted')}",
        "",
    ]
    gate = attempt.get("gate") or {}
    if gate.get("delta_percent") is not None:
        md.append(
            f"- bench nps: {gate.get('baseline_mean')} → {gate.get('current_mean')} "
            f"({gate.get('delta_percent'):+.3f}%, {gate.get('delta_nps'):+.0f})"
        )
    for reason in gate.get("reasons") or []:
        md.append(f"- {reason}")
    if attempt.get("hotspot_quality"):
        md.append(f"- hotspot quality: {attempt['hotspot_quality']}")
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
        "# Stockfish Orbit loop log",
        "",
        "| id | mode | proposal | decision | Δ nps % | when |",
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
    """CI-safe: exercise gate accept + reject without touching stockfish/ or make."""
    fixtures = [
        {
            "slug_suffix": "mock-reject-below-floor",
            "baseline_mean": 1_000_000.0,
            "current_mean": 1_002_000.0,
            "baseline_stdev": 15_000.0,
            "expect": "reject",
        },
        {
            "slug_suffix": "mock-reject-inside-sigma",
            "baseline_mean": 1_000_000.0,
            "current_mean": 1_006_000.0,
            "baseline_stdev": 15_000.0,
            "expect": "reject",
        },
        {
            "slug_suffix": "mock-accept-above-sigma",
            "baseline_mean": 1_000_000.0,
            "current_mean": 1_020_000.0,
            "baseline_stdev": 15_000.0,
            "expect": "accept",
        },
        {
            "slug_suffix": "mock-reject-fingerprint",
            "baseline_mean": 1_000_000.0,
            "current_mean": 1_050_000.0,
            "baseline_stdev": 1_000.0,
            "baseline_nodes": 1648567,
            "current_nodes": 1648568,
            "expect": "reject",
        },
    ]
    results = []
    failed = False
    for fixture in fixtures:
        gate = evaluate_gate(
            baseline_mean=fixture["baseline_mean"],
            current_mean=fixture["current_mean"],
            baseline_stdev=fixture["baseline_stdev"],
            min_gain_percent=min_gain_percent,
            sigma=sigma,
            baseline_nodes=fixture.get("baseline_nodes"),
            current_nodes=fixture.get("current_nodes"),
        )
        ok = gate["decision"] == fixture["expect"]
        if not ok:
            failed = True
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
            "hotspot_reason": "mock fixture; no capture",
        }
        dest = _write_attempt(log_dir, f"{ident}-{fixture['slug_suffix']}", attempt)
        results.append({"dir": str(dest), "ok": ok, "gate": gate, "expect": fixture["expect"]})
    _rewrite_index(log_dir)
    return {"ok": not failed, "mode": "mock", "attempts": results, "log_dir": str(log_dir)}


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    log_dir = Path(args.log_dir)
    log_dir.mkdir(parents=True, exist_ok=True)
    leftover = ops.STOCKFISH / "src" / "misc.cpp"
    if leftover.is_file() and "stockfish-orbit-loop" in leftover.read_text():
        ops.revert_stockfish(["src/misc.cpp"])

    ident = _next_log_id(log_dir)
    slug = f"{ident}-live-{args.proposal}"
    attempt: dict[str, Any] = {
        "id": ident,
        "mode": "live",
        "timestamp": _now(),
        "proposal_kind": args.proposal,
        "applied": False,
        "reverted": False,
        "decision": "reject",
    }

    if args.hotspots_from:
        spots = ops.inspect_hotspots(limit=15, summary_path=args.hotspots_from)
    else:
        spots = ops.inspect_hotspots(limit=15)
    classification = classify_hotspots(list(spots.get("hotspots") or []))
    attempt["hotspots"] = spots.get("hotspots") or []
    attempt["hotspot_backend"] = spots.get("backend")
    attempt["hotspot_quality"] = classification["quality"]
    attempt["hotspot_reason"] = classification["reason"]
    attempt["hotspot_classification"] = classification

    if args.proposal == "auto":
        kind = "noop_experiment"
    elif args.proposal == "noop":
        kind = "noop_experiment"
    else:
        kind = args.proposal
    attempt["proposal_kind"] = kind

    if kind != "noop_experiment":
        attempt["error"] = f"unsupported live proposal {kind}; use auto/noop"
        attempt["gate"] = evaluate_gate(baseline_mean=None, current_mean=None, baseline_stdev=None)
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}

    proposal = propose_noop(ident)
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
        out = args.baseline_out or str(ops.RUNS / f"{ident}-baseline")
        ran = ops.run_suite(
            out=out,
            speedtest_seconds=args.speedtest_seconds,
            bench_iters=args.bench_iters,
            perft=args.perft,
            capture=args.capture,
            skip_speedtest=args.skip_speedtest,
        )
        attempt["baseline_suite"] = {
            "ok": ran.get("ok"),
            "summary_path": ran.get("summary_path"),
            "bench_nps": ran.get("bench_nps"),
        }
        baseline = ops.load_summary(ran.get("summary_path") or out)
    if not baseline.get("ok"):
        attempt["error"] = baseline.get("error") or "baseline suite failed"
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}
    attempt["baseline"] = _bench_stats(baseline)

    applied = ops.apply_stockfish_patch(
        path=proposal["target_path"],
        content=proposal["new_content"],
        apply=True,
    )
    attempt["applied"] = bool(applied.get("applied"))
    attempt["apply"] = {"ok": applied.get("ok"), "diff": applied.get("diff")}
    if not applied.get("ok"):
        attempt["error"] = applied.get("error")
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}

    rebuild = ops.rebuild_stockfish(mode=args.rebuild_mode)
    attempt["rebuild"] = {
        "ok": rebuild.get("ok"),
        "command": rebuild.get("command"),
        "returncode": rebuild.get("returncode"),
        "stderr_tail": rebuild.get("stderr_tail"),
    }
    if not rebuild.get("ok"):
        revert = ops.revert_stockfish([proposal["target_path"]])
        attempt["reverted"] = True
        attempt["revert"] = revert
        attempt["error"] = "rebuild failed"
        _write_attempt(log_dir, slug, attempt)
        _rewrite_index(log_dir)
        return {"ok": False, **attempt}

    after_out = args.after_out or str(ops.RUNS / f"{ident}-after")
    after_run = ops.run_suite(
        out=after_out,
        baseline=str(baseline.get("_path")),
        speedtest_seconds=args.speedtest_seconds,
        bench_iters=args.bench_iters,
        perft=args.perft,
        capture="none",
        skip_speedtest=args.skip_speedtest,
    )
    after = ops.load_summary(after_run.get("summary_path") or after_out)
    attempt["after_suite"] = {
        "ok": after_run.get("ok"),
        "summary_path": after_run.get("summary_path"),
        "bench_nps": after_run.get("bench_nps"),
        "compare": after_run.get("compare"),
    }
    attempt["after"] = _bench_stats(after)

    b = attempt["baseline"]
    a = attempt["after"]
    gate = evaluate_gate(
        baseline_mean=b.get("mean"),
        current_mean=a.get("mean"),
        baseline_stdev=b.get("stdev"),
        min_gain_percent=args.min_gain_percent,
        sigma=args.sigma,
        baseline_nodes=b.get("nodes"),
        current_nodes=a.get("nodes"),
        perft_passed=(after.get("perft") or {}).get("passed"),
    )
    attempt["gate"] = gate
    attempt["decision"] = gate["decision"]

    if gate["accepted"]:
        attempt["reverted"] = False
        attempt["kept"] = (
            "Accepted by the nps gate. Change remains in the stockfish/ working "
            "tree only — not committed, not pushed upstream."
        )
    else:
        revert = ops.revert_stockfish([proposal["target_path"]])
        attempt["reverted"] = True
        attempt["revert"] = revert
        rebuild_back = ops.rebuild_stockfish(mode=args.rebuild_mode)
        attempt["rebuild_after_revert"] = {
            "ok": rebuild_back.get("ok"),
            "returncode": rebuild_back.get("returncode"),
        }

    dest = _write_attempt(log_dir, slug, attempt)
    _rewrite_index(log_dir)
    attempt["ok"] = True
    attempt["log_dir"] = str(dest)
    return attempt


def run_loop(**kwargs: Any) -> dict[str, Any]:
    """Programmatic entry used by the MCP tool."""
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
    parser.add_argument("--log-dir", default=str(LOG_DIR))
    parser.add_argument("--baseline", help="Reuse an existing summary.json as baseline")
    parser.add_argument("--baseline-out", help="Output dir for a freshly run baseline")
    parser.add_argument("--after-out", help="Output dir for the after-patch suite")
    parser.add_argument("--hotspots-from", help="summary.json to classify (default: latest)")
    parser.add_argument("--bench-iters", type=int, default=10)
    parser.add_argument("--speedtest-seconds", type=int, default=20)
    parser.add_argument("--skip-speedtest", action="store_true", default=True)
    parser.add_argument("--run-speedtest", action="store_true", help="Include speedtest (slow)")
    parser.add_argument("--perft", choices=("smoke", "official", "off"), default="off")
    parser.add_argument("--capture", choices=("auto", "orbit", "perf", "all", "none"), default="none")
    parser.add_argument("--rebuild-mode", choices=("incremental", "build", "profile-build"), default="incremental")
    parser.add_argument("--min-gain-percent", type=float, default=DEFAULT_MIN_GAIN_PERCENT)
    parser.add_argument("--sigma", type=float, default=DEFAULT_SIGMA)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.run_speedtest:
        args.skip_speedtest = False
    args.log_dir = Path(args.log_dir)
    args.log_dir.mkdir(parents=True, exist_ok=True)
    if args.mode == "mock":
        result = run_mock(args.log_dir, args.min_gain_percent, args.sigma)
    else:
        result = run_live(args)
    print(json.dumps(result, indent=2, default=str))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
