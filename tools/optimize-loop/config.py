# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Load an optimize-loop target config (JSON, TOML, or YAML)."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONFIG = ROOT / "docs" / "optimize-loop" / "examples" / "fixture.toml"


def load_raw(path: Path) -> dict[str, Any]:
    text = path.read_text()
    suffix = path.suffix.lower()
    if suffix == ".json":
        data = json.loads(text)
    elif suffix == ".toml":
        import tomllib

        data = tomllib.loads(text)
    elif suffix in (".yaml", ".yml"):
        try:
            import yaml  # type: ignore

            data = yaml.safe_load(text)
        except ImportError as exc:
            raise SystemExit(
                f"{path}: YAML needs PyYAML, or use .toml / .json instead"
            ) from exc
    else:
        raise SystemExit(f"unsupported config suffix {suffix} (use .toml, .json, .yaml)")
    if not isinstance(data, dict):
        raise SystemExit(f"{path}: config root must be an object")
    return data


def _as_cmd(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [str(x) for x in value]
    raise SystemExit(f"command must be a string or list, got {type(value)}")


def _as_metric(block: Any) -> dict[str, Any]:
    if not block:
        return {"regex": r"ORBIT_METRIC\s+\w+=(?P<value>[-+0-9.eE]+)", "name": "throughput"}
    if not isinstance(block, dict):
        raise SystemExit("metric must be a table")
    return {
        "regex": block.get("regex"),
        "json_path": block.get("json_path"),
        "name": block.get("name") or "throughput",
        "unit": block.get("unit") or "",
        "last_float": bool(block.get("last_float")),
    }


class TargetConfig:
    def __init__(self, data: dict[str, Any], config_path: Path):
        self.raw = data
        self.config_path = config_path
        self.name = str(data.get("name") or "target")
        project = data.get("project") or "."
        project_path = Path(project).expanduser()
        if not project_path.is_absolute():
            # Relative project: from repo root if it exists there, else from the config file.
            from_root = (ROOT / project_path).resolve()
            from_cfg = (config_path.parent / project_path).resolve()
            project_path = from_root if from_root.exists() else from_cfg
        self.project = project_path
        self.patch_root = (self.project / str(data.get("patch_root") or ".")).resolve()
        self.env = {str(k): str(v) for k, v in (data.get("env") or {}).items()}
        self.binary = str(data.get("binary") or "")

        build = data.get("build") or {}
        self.build_cmd = _as_cmd(build.get("command"))
        self.build_workdir = self._wd(build.get("workdir"))
        self.build_timeout = float(build.get("timeout_sec") or 1800)

        bench = data.get("bench") or {}
        self.bench_cmd = _as_cmd(bench.get("command"))
        if not self.bench_cmd:
            raise SystemExit("config.bench.command is required")
        self.bench_workdir = self._wd(bench.get("workdir"))
        self.bench_repeat = int(bench.get("repeat") or 10)
        self.bench_timeout = float(bench.get("timeout_sec") or 300)
        self.bench_metric = _as_metric(bench.get("metric"))

        corr = data.get("correctness") or {}
        self.correctness_cmd = _as_cmd(corr.get("command"))
        self.correctness_workdir = self._wd(corr.get("workdir"))
        self.correctness_timeout = float(corr.get("timeout_sec") or 300)
        self.correctness_expect = int(corr.get("expect_returncode") or 0)

        secondary = data.get("secondary") or {}
        self.secondary_cmd = _as_cmd(secondary.get("command"))
        self.secondary_workdir = self._wd(secondary.get("workdir"))
        self.secondary_timeout = float(secondary.get("timeout_sec") or 300)
        self.secondary_metric = _as_metric(secondary.get("metric")) if self.secondary_cmd else None

        capture = data.get("capture") or {}
        self.capture_cmd = _as_cmd(capture.get("command")) or list(self.bench_cmd)
        self.capture_workdir = self._wd(capture.get("workdir") or bench.get("workdir"))
        self.capture_duration_ms = int(capture.get("duration_ms") or 4000)

    def _wd(self, value: Any) -> Path:
        raw = Path(value) if value else Path(".")
        if raw.is_absolute():
            return raw
        return (self.project / raw).resolve()


def load_config(path: str | Path | None = None) -> TargetConfig:
    cfg_path = Path(path).expanduser() if path else DEFAULT_CONFIG
    if not cfg_path.is_absolute():
        candidate = (Path.cwd() / cfg_path).resolve()
        cfg_path = candidate if candidate.is_file() else (ROOT / cfg_path).resolve()
    if not cfg_path.is_file():
        raise SystemExit(f"config not found: {cfg_path}")
    return TargetConfig(load_raw(cfg_path), cfg_path)
