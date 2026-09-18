#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Stdio MCP server for the Stockfish Orbit poster loop (Phase 3).

This Orbit repo has no `orbit mcp` CLI (see docs/TODO.md item 12: the MCP
layer is explicitly not done). This process is the stockfish-specific
surface: JSON-RPC 2.0 over stdin/stdout, newline-delimited, as in
MCP 2024-11-05.

  python3 tools/stockfish-orbit-loop/mcp/server.py              # stdio MCP
  python3 tools/stockfish-orbit-loop/mcp/server.py --list-tools
  python3 tools/stockfish-orbit-loop/mcp/server.py --call NAME [JSON]

It does not call Claude/OpenAI/Grok and does not need API keys. The agent
host (Cursor, Claude Desktop, …) holds those.
"""

from __future__ import annotations

import argparse
import json
import sys
import traceback
from pathlib import Path
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ops  # noqa: E402

SERVER_NAME = "stockfish-orbit-loop"
SERVER_VERSION = "0.3.0"
PROTOCOL = "2024-11-05"

ToolFn = Callable[[dict[str, Any]], dict[str, Any]]

TOOLS: list[dict[str, Any]] = [
    {
        "name": "stockfish_run_suite",
        "description": (
            "Run the Stockfish Orbit profiling+benchmark suite "
            "(tools/stockfish-orbit-loop/run_suite.py): speedtest, repeated bench, "
            "perft, and optional Orbit/perf capture. Writes summary.json + summary.md."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "out": {"type": "string", "description": "Output directory for this run"},
                "baseline": {"type": "string", "description": "Previous summary.json for deltas"},
                "speedtest_seconds": {"type": "integer", "description": "Suite default 20; official is 150"},
                "bench_iters": {"type": "integer", "description": "Default 20"},
                "perft": {"type": "string", "enum": ["smoke", "official", "off"]},
                "capture": {"type": "string", "enum": ["auto", "orbit", "perf", "all", "none"]},
                "skip_speedtest": {"type": "boolean"},
                "skip_bench": {"type": "boolean"},
            },
        },
    },
    {
        "name": "stockfish_get_summary",
        "description": (
            "Fetch the latest structured suite summary JSON (runs/latest or newest "
            "docs/stockfish-orbit-loop/runs/*/summary.json). Optional path override."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "summary.json or a run directory"},
            },
        },
    },
    {
        "name": "stockfish_inspect_hotspots",
        "description": (
            "Return the top symbols / hotspot table from the latest (or given) "
            "suite summary's profile section."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "description": "Max rows (default 15)"},
                "path": {"type": "string", "description": "summary.json override"},
            },
        },
    },
    {
        "name": "stockfish_apply_patch",
        "description": (
            "Propose or apply a file edit inside the stockfish/ submodule only. "
            "Default is dry-run (apply=false): returns the unified diff, does not write. "
            "Set apply=true to write the working tree. Never git commit, push, or force-push."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path relative to stockfish/"},
                "content": {"type": "string", "description": "New file contents (with path)"},
                "unified_diff": {"type": "string", "description": "Instead of path+content, a git-apply patch"},
                "apply": {"type": "boolean", "description": "false=preview only (default); true=write tree"},
            },
        },
    },
    {
        "name": "stockfish_rebuild",
        "description": (
            "Rebuild Stockfish from stockfish/src via the upstream Makefile. "
            "mode=profile-build (recommended) or build (skip PGO)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": ["profile-build", "build"], "default": "profile-build"},
                "jobs": {"type": "integer", "description": "make -j N (default: nproc)"},
            },
        },
    },
    {
        "name": "stockfish_rerun_compare",
        "description": (
            "Re-run the bench suite against a baseline summary and return before/after "
            "nps deltas. Baseline defaults to the latest previous summary."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "baseline": {"type": "string", "description": "Baseline summary.json (default: latest)"},
                "out": {"type": "string"},
                "speedtest_seconds": {"type": "integer"},
                "bench_iters": {"type": "integer"},
                "perft": {"type": "string", "enum": ["smoke", "official", "off"]},
                "capture": {"type": "string", "enum": ["auto", "orbit", "perf", "all", "none"]},
            },
        },
    },
    {
        "name": "stockfish_run_loop",
        "description": (
            "Phase 4 closed loop: baseline suite, inspect hotspots, propose a small "
            "stockfish/ experiment (auto = no-op when capture is libc/startup), "
            "rebuild, compare, accept only if mean nps gain > 0.5% and outside 1σ. "
            "mode=mock is CI-safe (no make / no tree writes). Never git push."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": ["mock", "live"], "default": "mock"},
                "proposal": {"type": "string", "enum": ["auto", "noop"]},
                "baseline": {"type": "string"},
                "hotspots_from": {"type": "string"},
                "bench_iters": {"type": "integer"},
                "min_gain_percent": {"type": "number"},
                "sigma": {"type": "number"},
                "rebuild_mode": {"type": "string", "enum": ["incremental", "build", "profile-build"]},
                "log_dir": {"type": "string", "description": "Attempt log directory"},
            },
        },
    },
]


def _dispatch(name: str, arguments: dict[str, Any] | None) -> dict[str, Any]:
    args = arguments or {}
    if name == "stockfish_run_suite":
        return ops.run_suite(
            out=args.get("out"),
            baseline=args.get("baseline"),
            speedtest_seconds=args.get("speedtest_seconds"),
            bench_iters=args.get("bench_iters"),
            perft=args.get("perft"),
            capture=args.get("capture"),
            skip_speedtest=bool(args.get("skip_speedtest")),
            skip_bench=bool(args.get("skip_bench")),
        )
    if name == "stockfish_get_summary":
        return ops.load_summary(args.get("path"))
    if name == "stockfish_inspect_hotspots":
        return ops.inspect_hotspots(limit=int(args.get("limit") or 15), summary_path=args.get("path"))
    if name == "stockfish_apply_patch":
        return ops.apply_stockfish_patch(
            path=args.get("path"),
            content=args.get("content"),
            unified_diff=args.get("unified_diff"),
            apply=bool(args.get("apply")),
        )
    if name == "stockfish_rebuild":
        return ops.rebuild_stockfish(mode=args.get("mode") or "profile-build", jobs=args.get("jobs"))
    if name == "stockfish_rerun_compare":
        return ops.rerun_compare(
            baseline=args.get("baseline"),
            out=args.get("out"),
            speedtest_seconds=args.get("speedtest_seconds"),
            bench_iters=args.get("bench_iters"),
            perft=args.get("perft") or "smoke",
            capture=args.get("capture") or "none",
        )
    if name == "stockfish_run_loop":
        sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
        import loop as orbit_loop  # noqa: WPS433

        return orbit_loop.run_loop(
            mode=args.get("mode") or "mock",
            proposal=args.get("proposal") or "auto",
            baseline=args.get("baseline"),
            hotspots_from=args.get("hotspots_from"),
            bench_iters=args.get("bench_iters"),
            min_gain_percent=args.get("min_gain_percent"),
            sigma=args.get("sigma"),
            rebuild_mode=args.get("rebuild_mode"),
            log_dir=args.get("log_dir"),
        )
    return {"ok": False, "error": f"unknown tool: {name}"}


def _ok(id_: Any, result: Any) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id_, "result": result}


def _err(id_: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id_, "error": {"code": code, "message": message}}


_KNOWN_PROTOCOLS = ("2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25")


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
                    "Stockfish Orbit loop tools. Run a suite, read summaries/hotspots, "
                    "preview or apply patches only under stockfish/, rebuild, and compare. "
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
        arguments = params.get("arguments") or {}
        try:
            payload = _dispatch(name, arguments)
        except Exception as exc:  # noqa: BLE001 — returned to the client
            payload = {"ok": False, "error": str(exc), "traceback": traceback.format_exc()[-1500:]}
        text = json.dumps(payload, indent=2, default=str)
        return _ok(
            id_,
            {
                "content": [{"type": "text", "text": text}],
                "isError": not bool(payload.get("ok", True)),
            },
        )
    if id_ is None:
        return None
    return _err(id_, -32601, f"method not found: {method}")


def _read_rpc() -> tuple[dict[str, Any], str] | None:
    """Accept newline-delimited JSON or LSP-style Content-Length frames.

    The 2024-11-05 MCP spec describes newline-delimited JSON-RPC. Cursor and the
    official TypeScript/Python SDKs speak Content-Length. Both work here.
    """
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
    parser.add_argument("--call", nargs="+", metavar=("NAME", "JSON"), help="invoke one tool and print JSON")
    args = parser.parse_args(argv)
    if args.list_tools:
        print(json.dumps({"tools": [{"name": t["name"], "description": t["description"]} for t in TOOLS]}, indent=2))
        return 0
    if args.call:
        name = args.call[0]
        extra = json.loads(args.call[1]) if len(args.call) > 1 else {}
        payload = _dispatch(name, extra)
        print(json.dumps(payload, indent=2, default=str))
        return 0 if payload.get("ok", True) else 1
    serve_stdio()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
