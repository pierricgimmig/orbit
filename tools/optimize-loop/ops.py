# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Operations behind the optimize-loop MCP tools. Never git push."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import config as cfgmod  # noqa: E402
import suite  # noqa: E402
from config import ROOT, TargetConfig  # noqa: E402

_DIFF_PATH = re.compile(r"^(?:---|\+\+\+) ([^\t\n]+)")
RUNS = suite.RUNS


def _run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    timeout: float | None = None,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=cwd,
        timeout=timeout,
        check=False,
        text=True,
        capture_output=True,
        input=input_text,
    )


def resolve_target(config: str | None = None) -> TargetConfig:
    return cfgmod.load_config(config)


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
        return {"ok": False, "error": "no suite summary found; run optimize_run_suite first", "looked_in": str(RUNS)}
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
        "target_name": summary.get("target_name"),
        "project": summary.get("project"),
        "backend": backend,
        "samples": samples,
        "source": source,
        "note": profile.get("note") or profile.get("reason"),
        "hotspots": hotspots[: max(1, min(limit, 50))],
    }


def run_suite(
    *,
    config: str | None = None,
    out: str | None = None,
    baseline: str | None = None,
    bench_iters: int | None = None,
    capture: str | None = None,
    skip_bench: bool = False,
    skip_correctness: bool = False,
    skip_build: bool = False,
) -> dict[str, Any]:
    target = resolve_target(config)
    summary = suite.run_suite(
        target,
        out=Path(out) if out else None,
        baseline=Path(baseline) if baseline else None,
        bench_iters=bench_iters,
        capture_mode=capture or "auto",
        skip_bench=skip_bench,
        skip_correctness=skip_correctness,
        skip_build=skip_build,
    )
    return {
        "ok": bool((summary.get("bench") or {}).get("ok", True))
        and (summary.get("correctness") or {}).get("passed") is not False,
        "summary_path": summary.get("_path"),
        "target_name": summary.get("target_name"),
        "bench": summary.get("bench"),
        "correctness": summary.get("correctness"),
        "profile_backend": (summary.get("profile") or {}).get("backend"),
        "compare": summary.get("compare"),
    }


def rerun_compare(
    *,
    config: str | None = None,
    baseline: str | None = None,
    out: str | None = None,
    bench_iters: int | None = None,
    capture: str = "none",
    skip_build: bool = False,
) -> dict[str, Any]:
    base = latest_summary_path(baseline)
    if base is None:
        return {"ok": False, "error": "no baseline summary; pass baseline= or run optimize_run_suite first"}
    # skip_build matters: a re-measure of the *same* binary (noise batches, or
    # right after an explicit optimize_rebuild) must not rebuild, and an
    # incremental rebuild can silently keep objects from a previous build
    # variant (e.g. a profile-build), comparing a binary against itself.
    result = run_suite(
        config=config, out=out, baseline=str(base), bench_iters=bench_iters, capture=capture, skip_build=skip_build
    )
    result["baseline_path"] = str(base)
    return result


def rebuild(config: str | None = None) -> dict[str, Any]:
    target = resolve_target(config)
    return suite.run_build(target)


def _safe_project_path(target: TargetConfig, rel: str) -> Path:
    if not rel or rel.startswith("/") or rel.startswith("\\"):
        raise ValueError("path must be relative to the configured project")
    root = target.patch_root.resolve()
    candidate = (root / rel).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise ValueError(f"path escapes project {root}: {rel}") from exc
    if ".git" in candidate.parts:
        raise ValueError("refusing to touch .git")
    return candidate


def _git_diff(project: Path, paths: list[str] | None = None) -> str:
    return _run(["git", "-C", str(project), "diff", "--", *(paths or [])]).stdout


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


def apply_patch(
    *,
    config: str | None = None,
    path: str | None = None,
    content: str | None = None,
    unified_diff: str | None = None,
    apply: bool = False,
) -> dict[str, Any]:
    target = resolve_target(config)
    if unified_diff and (path or content is not None):
        return {"ok": False, "error": "pass unified_diff OR path+content, not both"}
    if not unified_diff and not path:
        return {"ok": False, "error": "need path+content or unified_diff"}
    status = _run(["git", "-C", str(target.project), "status", "--short", "--branch"])
    result: dict[str, Any] = {
        "ok": True,
        "applied": False,
        "dry_run": not apply,
        "pushed": False,
        "note": "Never commits or force-pushes. apply=true writes the project working tree.",
        "git_status_before": status.stdout.strip(),
        "project": str(target.project),
    }
    if unified_diff:
        try:
            for rel in _paths_in_unified_diff(unified_diff):
                _safe_project_path(target, rel)
        except ValueError as error:
            return {"ok": False, "error": str(error), "diff": unified_diff}
        check = _run(
            ["git", "-C", str(target.project), "apply", "--check", "--verbose", "-"],
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
        applied = _run(["git", "-C", str(target.project), "apply", "-"], input_text=unified_diff)
        result["applied"] = applied.returncode == 0
        result["ok"] = applied.returncode == 0
        result["diff_after"] = _git_diff(target.project)
        return result

    try:
        dest = _safe_project_path(target, path or "")
    except ValueError as error:
        return {"ok": False, "error": str(error)}
    if content is None:
        return {"ok": False, "error": "content is required when path is set"}
    dest_rel = dest.relative_to(target.project.resolve()).as_posix()
    old = dest.read_text() if dest.is_file() else ""
    import difflib

    new_text = content if content.endswith("\n") or content == "" else content + "\n"
    result["path"] = dest_rel
    result["diff"] = "".join(
        difflib.unified_diff(
            old.splitlines(keepends=True),
            new_text.splitlines(keepends=True),
            fromfile=f"a/{dest_rel}",
            tofile=f"b/{dest_rel}",
        )
    )
    if not apply:
        return result
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(new_text)
    result["applied"] = True
    result["diff_after"] = _git_diff(target.project, [dest_rel])
    return result


def revert_project(config: str | None = None, paths: list[str] | None = None) -> dict[str, Any]:
    """Restore listed files via git checkout. Never git clean -fd, never push."""
    target = resolve_target(config)
    restored: list[str] = []
    if paths:
        for rel in paths:
            dest = _safe_project_path(target, rel)
            rel_posix = dest.relative_to(target.project.resolve()).as_posix()
            checkout = _run(["git", "-C", str(target.project), "checkout", "--", rel_posix])
            if checkout.returncode == 0:
                restored.append(rel_posix)
                continue
            try:
                via_orbit = dest.resolve().relative_to(ROOT.resolve()).as_posix()
            except ValueError:
                via_orbit = None
            if via_orbit:
                _run(["git", "-C", str(ROOT), "checkout", "--", via_orbit])
                restored.append(rel_posix)
    else:
        _run(["git", "-C", str(target.project), "checkout", "--", "."])
    status = _run(["git", "-C", str(target.project), "status", "--short"])
    return {
        "ok": True,
        "restored": restored,
        "git_status": status.stdout.strip(),
        "pushed": False,
        "note": "Working tree restored. Nothing was committed or pushed.",
    }
