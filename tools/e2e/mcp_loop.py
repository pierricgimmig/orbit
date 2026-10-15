#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Drive a full profile through orbit-service mcp.

Starts orbit-service and the Frida sample target, then speaks JSON-RPC to
`orbit-service mcp`: attach, hook one function, capture, read timing, unhook,
capture again. Sampling is requested only where orbit_capabilities says it
exists; elsewhere the tool must refuse it without starting a capture.
"""
import argparse
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import time


def summarize(name, payload):
    """One line an agent log can show. The assertions use the payload, not this text."""
    if name == "orbit_capabilities":
        features = payload.get("features") or {}
        supported = [key for key, value in features.items() if isinstance(value, dict) and value.get("supported")]
        return f"platform={payload.get('platform')} reachable={payload.get('service_reachable')} supported={supported}"
    if name == "orbit_attach":
        return f"pid={payload.get('pid')} functions={payload.get('function_count')} modules={payload.get('module_count')}"
    if name == "orbit_search_functions":
        shown = ", ".join(
            f"{row.get('name')} id={row.get('function_id')} module={row.get('module')}"
            for row in (payload.get("functions") or [])[:3]
        )
        return f"pid={payload.get('pid')} {shown}"
    if name == "orbit_arm_hooks":
        shown = ", ".join(
            f"{row.get('name')} ({row.get('module')})" for row in (payload.get("hooks") or [])
        )
        return f"armed={payload.get('armed')} {shown}".rstrip()
    if name == "orbit_start_capture":
        return (
            f"started={payload.get('started')} hooks={payload.get('hooks')} "
            f"names={payload.get('hook_names')} sampling={payload.get('sampling')} "
            f"instrumentation={payload.get('instrumentation')}"
        )
    if name == "orbit_stop_capture":
        return "stopped"
    if name == "orbit_function_stats":
        rows = payload.get("rows") or []
        if not rows:
            return f"events_scanned={payload.get('events_scanned')} rows=0"
        row = rows[0]
        return (
            f"kind={row.get('kind')} name={row.get('name')} count={row.get('count')} "
            f"avg_ns={row.get('avg_ns')} total_ns={row.get('total_ns')}"
        )
    if name == "orbit_sampling_report":
        return (
            f"supported={payload.get('supported')} samples={payload.get('samples')} "
            f"hotspots={len(payload.get('hotspots') or [])}"
        )
    if name == "orbit_callstacks":
        return (
            f"supported={payload.get('supported')} samples={payload.get('samples')} "
            f"stacks={len(payload.get('stacks') or [])}"
        )
    if name == "orbit_unhook":
        return f"removed={payload.get('removed')} armed={payload.get('armed')}"
    return json.dumps(payload)[:180]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--service", required=True)
    parser.add_argument("--agent", required=True)
    parser.add_argument("--helper", required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    env = dict(
        os.environ,
        ORBIT_FRIDA_AGENT=str(Path(args.agent).resolve()),
        ORBIT_FRIDA_HELPER=str(Path(args.helper).resolve()),
    )
    with tempfile.TemporaryFile(mode="w+") as log:
        service = subprocess.Popen(
            [args.service, "--host", "127.0.0.1", "--serve", str(port)],
            env=env,
            stdout=log,
            stderr=log,
        )
        target = subprocess.Popen(
            [args.target],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=log,
            text=True,
        )
        mcp = None
        try:
            ready, _, _ = select.select([target.stdout], [], [], 10)
            assert ready, "target did not announce its pid"
            pid_line = target.stdout.readline().strip()
            assert pid_line.isdecimal(), pid_line
            pid = int(pid_line)
            deadline = time.monotonic() + 30
            while True:
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                        break
                except OSError:
                    assert service.poll() is None and time.monotonic() < deadline, "service exited"
                    time.sleep(0.05)
            mcp = subprocess.Popen(
                [args.service, "mcp", "--url", base],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            ids = iter(range(1, 1000))

            def rpc(method, params, note=True):
                message = {"jsonrpc": "2.0", "id": next(ids), "method": method, "params": params}
                mcp.stdin.write(json.dumps(message) + "\n")
                mcp.stdin.flush()
                line = mcp.stdout.readline()
                assert line, f"mcp exited: {mcp.poll()} stderr={mcp.stderr.read()[:500]!r}"
                reply = json.loads(line)
                assert "error" not in reply, reply
                return reply["result"]

            def tool(name, arguments):
                result = rpc("tools/call", {"name": name, "arguments": arguments})
                text = result["content"][0]["text"]
                payload = json.loads(text)
                assert not result.get("isError"), payload
                print(f"TOOL {name}: {summarize(name, payload)}", flush=True)
                return payload

            init = rpc(
                "initialize",
                {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "mcp_loop", "version": "0"}},
            )
            assert init["serverInfo"]["name"] == "orbit"
            mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
            mcp.stdin.flush()
            listed = rpc("tools/list", {})
            names = [item["name"] for item in listed["tools"]]
            for required in (
                "orbit_capabilities",
                "orbit_attach",
                "orbit_arm_hooks",
                "orbit_start_capture",
                "orbit_function_stats",
                "orbit_unhook",
                "orbit_sampling_report",
            ):
                assert required in names, names

            caps = tool("orbit_capabilities", {})
            sampling = caps["features"]["sampling"]["supported"]
            if not sampling:
                refused = tool("orbit_start_capture", {"pid": pid, "sampling": True, "scheduler": True})
                assert refused["started"] is False and refused["supported"] is False, refused
                report = tool("orbit_sampling_report", {})
                assert report["supported"] is False, report
                stacks = tool("orbit_callstacks", {})
                assert stacks["supported"] is False, stacks

            attached = tool("orbit_attach", {"pid": pid})
            assert attached["attached"] is True, attached
            found = tool("orbit_search_functions", {"query": "orbit_frida_test_inner", "limit": 10})
            hook = next(row for row in found["functions"] if row["name"].lstrip("_") == "orbit_frida_test_inner")
            armed = tool("orbit_arm_hooks", {"function_ids": [hook["function_id"]]})
            assert armed["armed"] == 1, armed
            assert armed["hooks"][0]["name"].lstrip("_") == "orbit_frida_test_inner", armed
            assert armed["hooks"][0]["module"], armed

            start_args = {}
            if sampling:
                start_args = {"sampling": True, "scheduler": True}
            started = tool("orbit_start_capture", start_args)
            assert started["started"] is True and started["hooks"] == 1, started
            assert started["instrumentation"] == "frida"

            target.stdin.write("go\n")
            target.stdin.flush()
            ready, _, _ = select.select([target.stdout], [], [], 20)
            assert ready and target.stdout.readline().strip() == "done", f"target stalled, exit={target.poll()}"
            time.sleep(0.2)
            tool("orbit_stop_capture", {})
            stats = tool("orbit_function_stats", {"query": "orbit_frida_test_inner", "pid": pid})
            rows = [row for row in stats["rows"] if row["kind"] == "hook"]
            assert rows and rows[0]["count"] > 0, stats
            if sampling:
                report = tool("orbit_sampling_report", {"limit": 5})
                assert report["supported"] is True and "hotspots" in report, report
                stacks = tool("orbit_callstacks", {"limit": 4})
                assert stacks["supported"] is True and "stacks" in stacks, stacks

            cleared = tool("orbit_unhook", {"function_ids": [hook["function_id"]]})
            assert cleared["armed"] == 0, cleared
            again = tool("orbit_start_capture", start_args)
            assert again["hooks"] == 0, again
            target.stdin.write("go\n")
            target.stdin.flush()
            ready, _, _ = select.select([target.stdout], [], [], 20)
            assert ready and target.stdout.readline().strip() == "done"
            time.sleep(0.2)
            tool("orbit_stop_capture", {})
            after = tool("orbit_function_stats", {"query": "orbit_frida_test_inner", "pid": pid})
            hooked = [row for row in after["rows"] if row["kind"] == "hook"]
            assert hooked == [], after
            print("PASS: attach, hook, capture, stats, unhook, recapture")
        finally:
            if mcp is not None:
                mcp.kill()
            target.kill()
            service.kill()
            target.wait(timeout=5)
            service.wait(timeout=5)


if __name__ == "__main__":
    main()
