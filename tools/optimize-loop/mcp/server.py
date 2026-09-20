#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Stdio MCP server for the generic optimize-loop.

This Orbit repo has no `orbit mcp` CLI (docs/TODO.md item 12). This process
is JSON-RPC 2.0 over stdin/stdout (MCP 2024-11-05), NDJSON and Content-Length.

  python3 -u tools/optimize-loop/mcp/server.py
  python3 tools/optimize-loop/mcp/server.py --list-tools
  python3 tools/optimize-loop/mcp/server.py --call optimize_get_summary

Does not call Claude/OpenAI/Grok. Keys stay on the host.
"""

from __future__ import annotations

import argparse
import json
import sys
import traceback
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import ops  # noqa: E402

SERVER_NAME = "orbit-optimize-loop"
SERVER_VERSION = "0.5.0"
PROTOCOL = "2024-11-05"
_KNOWN_PROTOCOLS = ("2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25")

TOOLS: list[dict[str, Any]] = [
    {
        "name": "optimize_run_suite",
        "description": (
            "Run the configured external project's bench/correctness/capture suite "
            "(tools/optimize-loop/suite.py). Writes summary.json + summary.md."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "config": {"type": "string", "description": "Path to target TOML/JSON/YAML"},
                "out": {"type": "string"},
                "baseline": {"type": "string"},
                "bench_iters": {"type": "integer"},
                "capture": {"type": "string", "enum": ["auto", "orbit", "none"]},
                "skip_bench": {"type": "boolean"},
                "skip_correctness": {"type": "boolean"},
                "skip_build": {"type": "boolean"},
            },
        },
    },
    {
        "name": "optimize_get_summary",
        "description": "Fetch the latest suite summary JSON (runs/latest or path=).",
        "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}},
    },
    {
        "name": "optimize_inspect_hotspots",
        "description": "Top symbols from the latest (or given) suite summary profile.",
        "inputSchema": {
            "type": "object",
            "properties": {"limit": {"type": "integer"}, "path": {"type": "string"}},
        },
    },
    {
        "name": "optimize_apply_patch",
        "description": (
            "Preview or apply a file edit inside the configured external project only. "
            "Default dry-run (apply=false). Never git commit, push, or force-push."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "config": {"type": "string"},
                "path": {"type": "string", "description": "Path relative to the project"},
                "content": {"type": "string"},
                "unified_diff": {"type": "string"},
                "apply": {"type": "boolean"},
            },
        },
    },
    {
        "name": "optimize_rebuild",
        "description": "Run the configured build.command in the external project.",
        "inputSchema": {"type": "object", "properties": {"config": {"type": "string"}}},
    },
    {
        "name": "optimize_rerun_compare",
        "description": "Re-run the suite against a baseline summary and return metric deltas.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "config": {"type": "string"},
                "baseline": {"type": "string"},
                "out": {"type": "string"},
                "bench_iters": {"type": "integer"},
                "capture": {"type": "string", "enum": ["auto", "orbit", "none"]},
            },
        },
    },
    {
        "name": "optimize_run_loop",
        "description": (
            "Closed loop: baseline, inspect hotspots, propose a no-op if capture is poor, "
            "rebuild, compare, accept only if mean gain > 0.5% and outside 1σ. "
            "mode=mock is CI-safe. Never git push."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": ["mock", "live"], "default": "mock"},
                "config": {"type": "string"},
                "proposal": {"type": "string", "enum": ["auto", "noop"]},
                "baseline": {"type": "string"},
                "hotspots_from": {"type": "string"},
                "bench_iters": {"type": "integer"},
                "min_gain_percent": {"type": "number"},
                "sigma": {"type": "number"},
                "log_dir": {"type": "string"},
            },
        },
    },
]


def _dispatch(name: str, arguments: dict[str, Any] | None) -> dict[str, Any]:
    args = arguments or {}
    if name == "optimize_run_suite":
        return ops.run_suite(
            config=args.get("config"),
            out=args.get("out"),
            baseline=args.get("baseline"),
            bench_iters=args.get("bench_iters"),
            capture=args.get("capture"),
            skip_bench=bool(args.get("skip_bench")),
            skip_correctness=bool(args.get("skip_correctness")),
            skip_build=bool(args.get("skip_build")),
        )
    if name == "optimize_get_summary":
        return ops.load_summary(args.get("path"))
    if name == "optimize_inspect_hotspots":
        return ops.inspect_hotspots(limit=int(args.get("limit") or 15), summary_path=args.get("path"))
    if name == "optimize_apply_patch":
        return ops.apply_patch(
            config=args.get("config"),
            path=args.get("path"),
            content=args.get("content"),
            unified_diff=args.get("unified_diff"),
            apply=bool(args.get("apply")),
        )
    if name == "optimize_rebuild":
        return ops.rebuild(args.get("config"))
    if name == "optimize_rerun_compare":
        return ops.rerun_compare(
            config=args.get("config"),
            baseline=args.get("baseline"),
            out=args.get("out"),
            bench_iters=args.get("bench_iters"),
            capture=args.get("capture") or "none",
        )
    if name == "optimize_run_loop":
        import loop as orbit_loop

        return orbit_loop.run_loop(
            mode=args.get("mode") or "mock",
            config=args.get("config"),
            proposal=args.get("proposal") or "auto",
            baseline=args.get("baseline"),
            hotspots_from=args.get("hotspots_from"),
            bench_iters=args.get("bench_iters"),
            min_gain_percent=args.get("min_gain_percent"),
            sigma=args.get("sigma"),
            log_dir=args.get("log_dir"),
        )
    return {"ok": False, "error": f"unknown tool: {name}"}


def _ok(id_: Any, result: Any) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id_, "result": result}


def _err(id_: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id_, "error": {"code": code, "message": message}}


def handle_message(msg: dict[str, Any]) -> dict[str, Any] | None:
    method = msg.get("method")
    id_ = msg.get("id")
    params = msg.get("params") or {}
    if method == "initialize":
        requested = params.get("protocolVersion") or PROTOCOL
        version = requested if requested in _KNOWN_PROTOCOLS else PROTOCOL
        return _ok(
            id_,
            {
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                "instructions": (
                    "Orbit optimize-loop tools for an external project from a config file. "
                    "Does not push to git. LLM API keys belong on the host, not this server."
                ),
            },
        )
    if method in ("notifications/initialized", "notifications/cancelled"):
        return None
    if method == "ping":
        return _ok(id_, {})
    if method == "tools/list":
        return _ok(id_, {"tools": TOOLS})
    if method == "resources/list":
        return _ok(id_, {"resources": []})
    if method == "prompts/list":
        return _ok(id_, {"prompts": []})
    if method == "tools/call":
        name = params.get("name")
        if not name:
            return _err(id_, -32602, "missing tool name")
        try:
            payload = _dispatch(name, params.get("arguments") or {})
        except Exception as exc:  # noqa: BLE001
            payload = {"ok": False, "error": str(exc), "traceback": traceback.format_exc()[-1500:]}
        return _ok(
            id_,
            {
                "content": [{"type": "text", "text": json.dumps(payload, indent=2, default=str)}],
                "isError": not bool(payload.get("ok", True)),
            },
        )
    if id_ is None:
        return None
    return _err(id_, -32601, f"method not found: {method}")


def _read_rpc() -> tuple[dict[str, Any], str] | None:
    raw = sys.stdin.buffer
    while True:
        line = raw.readline()
        if not line:
            return None
        if line.strip():
            break
    stripped = line.strip()
    if stripped.startswith(b"{") or stripped.startswith(b"["):
        return json.loads(stripped), "ndjson"
    headers: dict[str, str] = {}
    current = line
    while current and current not in (b"\r\n", b"\n"):
        if b":" in current:
            key, value = current.decode("ascii", errors="replace").split(":", 1)
            headers[key.strip().lower()] = value.strip()
        current = raw.readline()
        if not current:
            break
    length = int(headers.get("content-length", "0"))
    body = raw.read(length) if length else b""
    return json.loads(body), "content-length"


def _write_rpc(msg: dict[str, Any], framing: str) -> None:
    body = json.dumps(msg, separators=(",", ":")).encode("utf-8")
    if framing == "content-length":
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body)
    else:
        sys.stdout.buffer.write(body + b"\n")
    sys.stdout.buffer.flush()


def serve_stdio() -> None:
    while True:
        try:
            read = _read_rpc()
        except json.JSONDecodeError as exc:
            sys.stderr.write(f"invalid json: {exc}\n")
            continue
        if read is None:
            return
        msg, framing = read
        reply = handle_message(msg)
        if reply is not None:
            _write_rpc(reply, framing)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--list-tools", action="store_true")
    parser.add_argument("--call", nargs="+", metavar=("NAME", "JSON"))
    args = parser.parse_args(argv)
    if args.list_tools:
        print(json.dumps({"tools": [{"name": t["name"], "description": t["description"]} for t in TOOLS]}, indent=2))
        return 0
    if args.call:
        payload = _dispatch(args.call[0], json.loads(args.call[1]) if len(args.call) > 1 else {})
        print(json.dumps(payload, indent=2, default=str))
        return 0 if payload.get("ok", True) else 1
    serve_stdio()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
