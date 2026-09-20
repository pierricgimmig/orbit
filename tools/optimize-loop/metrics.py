# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Parse a primary throughput number from command output."""

from __future__ import annotations

import json
import re
from typing import Any

ORBIT_METRIC = re.compile(
    r"^ORBIT_METRIC\s+(?P<name>\w+)\s*=\s*(?P<value>[-+0-9.eE]+)\s*$",
    re.M,
)
ORBIT_FINGERPRINT = re.compile(r"^ORBIT_FINGERPRINT\s+(?P<value>\S+)\s*$", re.M)
LAST_FLOAT = re.compile(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?")


def _json_path(data: Any, path: str) -> Any:
    cur: Any = data
    for part in path.lstrip("$.").split("."):
        if not part:
            continue
        if isinstance(cur, dict):
            cur = cur.get(part)
        else:
            return None
    return cur


def parse_metric(text: str, spec: dict[str, Any] | None = None) -> dict[str, Any]:
    spec = spec or {}
    name = spec.get("name") or "throughput"
    value = None
    source = None

    if spec.get("json_path"):
        try:
            payload = json.loads(text)
        except json.JSONDecodeError:
            payload = None
        if payload is not None:
            raw = _json_path(payload, str(spec["json_path"]))
            if raw is not None:
                value = float(raw)
                source = f"json:{spec['json_path']}"

    if value is None and spec.get("regex"):
        match = re.search(spec["regex"], text)
        if match:
            groups = match.groupdict()
            raw = groups.get("value") or (match.group(1) if match.lastindex else None)
            if raw is not None:
                value = float(raw)
                name = groups.get("name") or name
                source = "regex"

    if value is None:
        match = ORBIT_METRIC.search(text)
        if match:
            value = float(match.group("value"))
            name = match.group("name")
            source = "ORBIT_METRIC"

    if value is None and spec.get("last_float"):
        found = LAST_FLOAT.findall(text)
        if found:
            value = float(found[-1])
            source = "last_float"

    fingerprint = None
    fp = ORBIT_FINGERPRINT.search(text)
    if fp:
        fingerprint = fp.group("value")

    return {
        "ok": value is not None,
        "name": name,
        "value": value,
        "unit": spec.get("unit") or "",
        "source": source,
        "fingerprint": fingerprint,
    }
