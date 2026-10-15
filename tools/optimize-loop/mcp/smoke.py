#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Smoke-test the optimize-loop MCP server and in-repo fixture."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SERVER = HERE / "server.py"
ROOT = Path(__file__).resolve().parents[3]
CONFIG = "docs/optimize-loop/examples/fixture.toml"


def _run(args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SERVER), *args],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=check,
    )


def _stdio(messages: list[dict], framing: str) -> list[dict]:
    payload = b""
    for msg in messages:
        body = json.dumps(msg, separators=(",", ":")).encode("utf-8")
        if framing == "content-length":
            payload += f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body
        else:
            payload += body + b"\n"
    proc = subprocess.run(
        [sys.executable, "-u", str(SERVER)],
        cwd=ROOT,
        input=payload,
        capture_output=True,
        check=True,
    )
    replies: list[dict] = []
    if framing == "content-length":
        data = proc.stdout
        while data.startswith(b"Content-Length:"):
            header, rest = data.split(b"\r\n\r\n", 1)
            length = int(header.split(b":", 1)[1].strip())
            body, data = rest[:length], rest[length:]
            replies.append(json.loads(body))
    else:
        for line in proc.stdout.splitlines():
            if line.strip():
                replies.append(json.loads(line))
    return replies


def main() -> int:
    listed = json.loads(_run(["--list-tools"]).stdout)
    names = [t["name"] for t in listed["tools"]]
    expected = [
        "optimize_run_suite",
        "optimize_get_summary",
        "optimize_inspect_hotspots",
        "optimize_apply_patch",
        "optimize_rebuild",
        "optimize_rerun_compare",
        "optimize_run_loop",
    ]
    missing = [n for n in expected if n not in names]
    if missing:
        print("missing tools:", missing, file=sys.stderr)
        return 1
    print("tools:", ", ".join(names))

    for framing in ("ndjson", "content-length"):
        replies = _stdio(
            [
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "smoke"}},
                },
                {"jsonrpc": "2.0", "method": "notifications/initialized"},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            ],
            framing,
        )
        if len(replies) != 2 or replies[1]["result"]["tools"][0]["name"] != expected[0]:
            print(f"{framing} handshake failed", file=sys.stderr)
            return 1
        print(f"{framing} initialize + tools/list: ok ({len(replies[1]['result']['tools'])} tools)")

    suite = json.loads(
        _run(
            [
                "--call",
                "optimize_run_suite",
                json.dumps(
                    {
                        "config": CONFIG,
                        "out": "/tmp/ol-mcp-smoke",
                        "bench_iters": 2,
                        "capture": "none",
                    }
                ),
            ]
        ).stdout
    )
    if not suite.get("ok"):
        print("run_suite failed:", suite, file=sys.stderr)
        return 1
    print(f"optimize_run_suite: ok mean={((suite.get('bench') or {}).get('mean'))}")

    got = json.loads(_run(["--call", "optimize_get_summary", json.dumps({"path": "/tmp/ol-mcp-smoke"})]).stdout)
    if not got.get("ok"):
        print("get_summary failed:", got, file=sys.stderr)
        return 1
    print(f"optimize_get_summary: ok path={got.get('_path')}")

    preview = json.loads(
        _run(
            [
                "--call",
                "optimize_apply_patch",
                json.dumps(
                    {
                        "config": CONFIG,
                        "path": "workload.py",
                        "content": "# dry-run only\n",
                        "apply": False,
                    }
                ),
            ]
        ).stdout
    )
    if not preview.get("ok") or preview.get("applied"):
        print("apply_patch dry-run failed:", preview, file=sys.stderr)
        return 1
    print("optimize_apply_patch dry-run: ok")

    escaped = _run(
        [
            "--call",
            "optimize_apply_patch",
            json.dumps({"config": CONFIG, "path": "../README.md", "content": "nope", "apply": False}),
        ],
        check=False,
    )
    if json.loads(escaped.stdout).get("ok"):
        print("escape should fail", escaped.stdout, file=sys.stderr)
        return 1
    print("optimize_apply_patch escape: refused")

    looped = json.loads(
        _run(["--call", "optimize_run_loop", json.dumps({"mode": "mock", "log_dir": "/tmp/ol-loop-mock"})]).stdout
    )
    if not looped.get("ok"):
        print("run_loop mock failed:", looped, file=sys.stderr)
        return 1
    print(f"optimize_run_loop mock: ok attempts={len(looped.get('attempts') or [])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
