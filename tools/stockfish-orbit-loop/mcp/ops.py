#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Operations behind the Stockfish Orbit loop MCP tools.

None of these call an LLM or read API keys. They wrap the Phase 2 harness
and git/make inside the `stockfish/` submodule. They never `git push`.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

_DIFF_PATH = re.compile(r"^(?:---|\+\+\+) ([^\t\n]+)")

ROOT = Path(__file__).resolve().parents[3]
STOCKFISH = ROOT / "stockfish"
STOCKFISH_SRC = STOCKFISH / "src"
RUNS = ROOT / "docs" / "stockfish-orbit-loop" / "runs"
SUITE = ROOT / "tools" / "stockfish-orbit-loop" / "run_suite.py"


def _run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    timeout: float | None = None,
    check: bool = False,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=cwd,
        timeout=timeout,
        check=check,
        text=True,
        capture_output=True,
        input=input_text,
    )


def latest_summary_path(explicit: str | None = None) -> Path | None:
    if explicit:
        path = Path(explicit).expanduser()
        if not path.is_absolute():
            path = (ROOT / path).resolve()
        if path.is_dir():
            path = path / "summary.json"
        return path if path.is_file() else None
    latest = RUNS / "latest" / "summary.json"
    if latest.is_file():
        return latest.resolve()
    pointer = RUNS / "latest.path"
    if pointer.is_file():
        candidate = Path(pointer.read_text().strip()) / "summary.json"
        if candidate.is_file():
            return candidate
    if RUNS.is_dir():
        found = sorted(RUNS.glob("*/summary.json"), key=lambda p: p.stat().st_mtime, reverse=True)
        if found:
            return found[0]
    return None


def load_summary(explicit: str | None = None) -> dict[str, Any]:
    path = latest_summary_path(explicit)
    if path is None:
        return {
            "ok": False,
            "error": "no suite summary found; run stockfish_run_suite first",
            "looked_in": str(RUNS),
        }
    data = json.loads(path.read_text())
    data["_path"] = str(path)
    data["ok"] = True
    return data


def inspect_hotspots(limit: int = 15, summary_path: str | None = None) -> dict[str, Any]:
    summary = load_summary(summary_path)
    if not summary.get("ok"):
        return summary
    profile = summary.get("profile") or {}
    hotspots = list(profile.get("hotspots") or [])
    backend = profile.get("backend")
    samples = profile.get("samples")
    source = "profile.hotspots"
    if not hotspots:
        for attempt in profile.get("attempts") or []:
            rows = attempt.get("hotspots") or []
            if rows:
                hotspots = list(rows)
                backend = attempt.get("backend") or backend
                samples = attempt.get("samples") if attempt.get("samples") is not None else samples
                source = f"profile.attempts[{attempt.get('backend')}]"
                break
    return {
        "ok": True,
        "summary_path": summary.get("_path"),
        "stockfish_sha": summary.get("stockfish_sha"),
        "backend": backend,
        "samples": samples,
        "source": source,
        "note": profile.get("note") or profile.get("reason"),
        "hotspots": hotspots[: max(1, min(limit, 50))],
    }


def run_suite(
    *,
    out: str | None = None,
    baseline: str | None = None,
    speedtest_seconds: int | None = None,
    bench_iters: int | None = None,
    perft: str | None = None,
    capture: str | None = None,
    skip_speedtest: bool = False,
    skip_bench: bool = False,
) -> dict[str, Any]:
    if not SUITE.is_file():
        return {"ok": False, "error": f"missing {SUITE}"}
    cmd = [sys.executable, str(SUITE)]
    if out:
        cmd += ["--out", out]
    if baseline:
        cmd += ["--baseline", baseline]
    if speedtest_seconds is not None:
        cmd += ["--speedtest-seconds", str(speedtest_seconds)]
    if bench_iters is not None:
        cmd += ["--bench-iters", str(bench_iters)]
    if perft:
        cmd += ["--perft", perft]
    if capture:
        cmd += ["--capture", capture]
    if skip_speedtest:
        cmd.append("--skip-speedtest")
    if skip_bench:
        cmd.append("--skip-bench")
    proc = _run(cmd, cwd=ROOT, timeout=4 * 3600)
    summary = load_summary(out)
    return {
        "ok": proc.returncode == 0,
        "returncode": proc.returncode,
        "command": cmd,
        "stdout_tail": (proc.stdout or "")[-2000:],
        "stderr_tail": (proc.stderr or "")[-1000:],
        "summary_path": summary.get("_path"),
        "speedtest_nps": (summary.get("speedtest") or {}).get("nps"),
        "bench_nps": (summary.get("bench") or {}).get("nps"),
        "perft_passed": (summary.get("perft") or {}).get("passed"),
        "profile_backend": (summary.get("profile") or {}).get("backend"),
        "compare": summary.get("compare"),
    }


def rerun_compare(
    *,
    baseline: str | None = None,
    out: str | None = None,
    speedtest_seconds: int | None = None,
    bench_iters: int | None = None,
    perft: str = "smoke",
    capture: str = "none",
) -> dict[str, Any]:
    base = latest_summary_path(baseline)
    if base is None:
        return {
            "ok": False,
            "error": "no baseline summary; pass baseline= or run stockfish_run_suite first",
        }
    result = run_suite(
        out=out,
        baseline=str(base),
        speedtest_seconds=speedtest_seconds,
        bench_iters=bench_iters,
        perft=perft,
        capture=capture,
    )
    result["baseline_path"] = str(base)
    return result


def revert_stockfish(paths: list[str] | None = None) -> dict[str, Any]:
    """Restore tracked files under stockfish/; delete untracked experiment files.

    Never git clean -fd (that would wipe NNUE nets and build outputs).
    Never commit or push.
    """
    stockfish = STOCKFISH.resolve()
    restored: list[str] = []
    removed: list[str] = []
    if paths:
        for rel in paths:
            dest = _safe_stockfish_path(rel)
            rel_posix = dest.relative_to(stockfish).as_posix()
            tracked = _run(["git", "-C", str(STOCKFISH), "ls-files", "--error-unmatch", rel_posix])
            if tracked.returncode == 0:
                checkout = _run(["git", "-C", str(STOCKFISH), "checkout", "--", rel_posix])
                if checkout.returncode != 0:
                    return {"ok": False, "error": checkout.stderr or checkout.stdout, "path": rel_posix}
                restored.append(rel_posix)
            elif dest.is_file():
                dest.unlink()
                removed.append(rel_posix)
    else:
        _run(["git", "-C", str(STOCKFISH), "checkout", "--", "."])
    status = _run(["git", "-C", str(STOCKFISH), "status", "--short"])
    return {
        "ok": True,
        "restored": restored,
        "removed": removed,
        "git_status": status.stdout.strip(),
        "pushed": False,
        "note": "Working tree restored. Nothing was committed or pushed.",
    }


def rebuild_stockfish(mode: str = "profile-build", jobs: int | None = None) -> dict[str, Any]:
    if mode not in ("profile-build", "build", "incremental"):
        return {"ok": False, "error": "mode must be profile-build, build, or incremental"}
    if not (STOCKFISH_SRC / "Makefile").is_file():
        return {
            "ok": False,
            "error": "stockfish/src/Makefile missing; git submodule update --init stockfish",
        }
    j = str(jobs or os.cpu_count() or 1)
    cmd = ["make", "-C", str(STOCKFISH_SRC), "-j", j]
    if mode != "incremental":
        cmd.append(mode)
    proc = _run(cmd, timeout=30 * 60)
    return {
        "ok": proc.returncode == 0,
        "returncode": proc.returncode,
        "command": cmd,
        "stdout_tail": (proc.stdout or "")[-2500:],
        "stderr_tail": (proc.stderr or "")[-1000:],
        "binary": str(STOCKFISH_SRC / "stockfish"),
        "binary_exists": (STOCKFISH_SRC / "stockfish").is_file(),
    }


def _safe_stockfish_path(rel: str) -> Path:
    if not rel or rel.startswith("/") or rel.startswith("\\"):
        raise ValueError("path must be relative to stockfish/")
    stockfish = STOCKFISH.resolve()
    candidate = (stockfish / rel).resolve()
    try:
        candidate.relative_to(stockfish)
    except ValueError as exc:
        raise ValueError(f"path escapes stockfish/: {rel}") from exc
    if ".git" in candidate.parts:
        raise ValueError("refusing to touch stockfish/.git")
    return candidate


def _git_diff(paths: list[str] | None = None) -> str:
    cmd = ["git", "-C", str(STOCKFISH), "diff", "--", *(paths or [])]
    return _run(cmd).stdout


def _paths_in_unified_diff(diff: str) -> list[str]:
    paths: list[str] = []
    for line in diff.splitlines():
        match = _DIFF_PATH.match(line)
        if not match:
            continue
        raw = match.group(1).strip()
        if raw == "/dev/null":
            continue
        if raw.startswith("a/") or raw.startswith("b/"):
            raw = raw[2:]
        paths.append(raw)
    return paths


def apply_stockfish_patch(
    *,
    path: str | None = None,
    content: str | None = None,
    unified_diff: str | None = None,
    apply: bool = False,
) -> dict[str, Any]:
    """Stage a file write or `git apply` inside stockfish/. Never commits or pushes.

    Default is dry-run (`apply=false`): compute/show the diff only.
    """
    if unified_diff and (path or content is not None):
        return {"ok": False, "error": "pass unified_diff OR path+content, not both"}
    if not unified_diff and not path:
        return {"ok": False, "error": "need path+content or unified_diff"}

    status = _run(["git", "-C", str(STOCKFISH), "status", "--short", "--branch"])
    result: dict[str, Any] = {
        "ok": True,
        "applied": False,
        "dry_run": not apply,
        "pushed": False,
        "note": "Never commits or force-pushes. Review the diff; apply=true writes the tree.",
        "git_status_before": status.stdout.strip(),
        "submodule": str(STOCKFISH),
    }

    if unified_diff:
        try:
            for rel in _paths_in_unified_diff(unified_diff):
                _safe_stockfish_path(rel)
        except ValueError as error:
            return {"ok": False, "error": str(error), "diff": unified_diff}
        check = _run(
            ["git", "-C", str(STOCKFISH), "apply", "--check", "--verbose", "-"],
            input_text=unified_diff,
        )
        result["git_apply_check_ok"] = check.returncode == 0
        result["git_apply_check"] = (check.stderr or check.stdout)[-1500:]
        result["diff"] = unified_diff
        if check.returncode != 0:
            result["ok"] = False
            result["error"] = "git apply --check failed"
            return result
        if not apply:
            return result
        applied = _run(
            ["git", "-C", str(STOCKFISH), "apply", "-"],
            input_text=unified_diff,
        )
        result["applied"] = applied.returncode == 0
        result["ok"] = applied.returncode == 0
        result["git_apply"] = (applied.stderr or applied.stdout)[-1500:]
        result["diff_after"] = _git_diff()
        return result

    try:
        dest = _safe_stockfish_path(path or "")
    except ValueError as error:
        return {"ok": False, "error": str(error)}
    if content is None:
        return {"ok": False, "error": "content is required when path is set"}
    dest_rel = dest.relative_to(STOCKFISH.resolve()).as_posix()
    old = dest.read_text() if dest.is_file() else ""
    import difflib

    new_text = content if content.endswith("\n") or content == "" else content + "\n"
    diff_text = "".join(
        difflib.unified_diff(
            old.splitlines(keepends=True),
            new_text.splitlines(keepends=True),
            fromfile=f"a/{dest_rel}",
            tofile=f"b/{dest_rel}",
        )
    )
    result["path"] = dest_rel
    result["diff"] = diff_text
    if not apply:
        return result
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(content if content.endswith("\n") or content == "" else content + "\n")
    result["applied"] = True
    result["diff_after"] = _git_diff([dest_rel])
    return result
