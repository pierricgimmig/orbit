#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Smoke-test the Stockfish Orbit MCP server (list tools + a few calls)."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SERVER = HERE / "server.py"
ROOT = Path(__file__).resolve().parents[3]


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
        while data:
            if data.startswith(b"Content-Length:"):
                header, rest = data.split(b"\r\n\r\n", 1)
                length = int(header.split(b":", 1)[1].strip())
                body, data = rest[:length], rest[length:]
                replies.append(json.loads(body))
            else:
                break
    else:
        for line in proc.stdout.splitlines():
            line = line.strip()
            if line:
                replies.append(json.loads(line))
    return replies


def main() -> int:
    listed = json.loads(_run(["--list-tools"]).stdout)
    names = [t["name"] for t in listed["tools"]]
    expected = [
        "stockfish_run_suite",
        "stockfish_get_summary",
        "stockfish_inspect_hotspots",
        "stockfish_apply_patch",
        "stockfish_rebuild",
        "stockfish_rerun_compare",
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
            print(f"{framing} handshake failed: {replies!r}", file=sys.stderr)
            return 1
        print(f"{framing} initialize + tools/list: ok ({len(replies[1]['result']['tools'])} tools)")

    summary_candidates = [
        Path("/tmp/sf-suite-5/summary.json"),
        Path("/tmp/sf-suite-2/summary.json"),
    ]
    summary = next((p for p in summary_candidates if p.is_file()), None)
    if summary is None:
        print("no existing summary.json for get_summary/hotspots smoke", file=sys.stderr)
        return 1

    got = json.loads(_run(["--call", "stockfish_get_summary", json.dumps({"path": str(summary)})]).stdout)
    if not got.get("ok") or "bench" not in got:
        print("get_summary failed:", got, file=sys.stderr)
        return 1
    print(f"stockfish_get_summary: ok path={got.get('_path')} sha={got.get('stockfish_sha')}")

    spots = json.loads(
        _run(["--call", "stockfish_inspect_hotspots", json.dumps({"path": str(summary), "limit": 5})]).stdout
    )
    if not spots.get("ok"):
        print("inspect_hotspots failed:", spots, file=sys.stderr)
        return 1
    print(f"stockfish_inspect_hotspots: backend={spots.get('backend')} rows={len(spots.get('hotspots') or [])}")

    preview = json.loads(
        _run(
            [
                "--call",
                "stockfish_apply_patch",
                json.dumps(
                    {
                        "path": "src/orbit_loop_note.txt",
                        "content": "Phase 3 MCP dry-run only. Do not apply.\n",
                        "apply": False,
                    }
                ),
            ]
        ).stdout
    )
    if not preview.get("ok") or preview.get("applied") or not preview.get("dry_run"):
        print("apply_patch dry-run failed:", preview, file=sys.stderr)
        return 1
    print("stockfish_apply_patch dry-run: ok (not written)")

    escaped = _run(
        ["--call", "stockfish_apply_patch", json.dumps({"path": "../README.md", "content": "nope", "apply": False})],
        check=False,
    )
    escaped_payload = json.loads(escaped.stdout)
    if escaped.returncode == 0 or escaped_payload.get("ok"):
        print("apply_patch should refuse path escape:", escaped_payload, file=sys.stderr)
        return 1
    print("stockfish_apply_patch escape: refused")

    suite = json.loads(
        _run(
            [
                "--call",
                "stockfish_run_suite",
                json.dumps(
                    {
                        "out": "/tmp/sf-mcp-smoke",
                        "skip_speedtest": True,
                        "bench_iters": 1,
                        "perft": "off",
                        "capture": "none",
                    }
                ),
            ]
        ).stdout
    )
    if not suite.get("ok"):
        print("run_suite smoke failed:", suite, file=sys.stderr)
        return 1
    print(
        f"stockfish_run_suite: ok bench_nps={suite.get('bench_nps')} "
        f"summary={suite.get('summary_path')}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
