<!--
Copyright (c) 2026 The Orbit Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Agents

`orbit-service mcp` is how an external agent drives a profile: list processes, attach, search functions, arm dynamic hooks, start and stop a capture, and read the results back. Claude Code, Cursor, and any other Model Context Protocol client spawn it. The capture stays in `orbit-service`. Closing the agent does not stop the service.

The tools do not edit source, rebuild, kill a process, or delete files. Hooking a function patches it in memory for the duration of a capture and puts the bytes back when the capture stops.

## Start the service

The MCP process is a client. The service has to be running first, on loopback:

```
orbit-service --host 127.0.0.1
```

That listens on `127.0.0.1:44766`. The HTTP API has no authentication. `--host 0.0.0.0`, the default when you pass `--serve` without `--host`, exposes attach and capture to the network. Leave it on `127.0.0.1` unless you mean to share it.

## Claude Code

`~/.claude.json`, or a project `.mcp.json`:

```json
{
  "mcpServers": {
    "orbit": {
      "command": "orbit-service",
      "args": ["mcp", "--url", "http://127.0.0.1:44766"]
    }
  }
}
```

## Cursor

`.cursor/mcp.json` in the project, or Cursor Settings → MCP:

```json
{
  "mcpServers": {
    "orbit": {
      "command": "orbit-service",
      "args": ["mcp", "--url", "http://127.0.0.1:44766"]
    }
  }
}
```

## Streamable HTTP

Clients that speak streamable HTTP instead of stdio:

```
orbit-service mcp --http 127.0.0.1:44767 --url http://127.0.0.1:44766
```

```json
{
  "mcpServers": {
    "orbit": {
      "url": "http://127.0.0.1:44767/mcp"
    }
  }
}
```

`--http` refuses any address that is not loopback. A foreign `Origin` is rejected. There is still no token: loopback is the boundary.

`--url http://10.0.0.1:44766` is refused unless you pass `--allow-remote`. That flag means anyone who can reach the service can attach to processes as the user running it.

## Tools

Call `orbit_capabilities` first. It reports the platform, whether the service is up, and which features this machine can actually record.

| Tool | What it does |
|---|---|
| `orbit_capabilities` | Platform, feature support, attached pid, armed hooks |
| `orbit_list_processes` | Processes, busiest first. `query`, `limit`, `offset` |
| `orbit_attach` | Load symbols for a pid and remember it |
| `orbit_search_functions` | One page of functions. Pass `function_id` to arm |
| `orbit_arm_hooks` | The set the next capture will instrument. Not installed yet |
| `orbit_unhook` | Remove functions from that set, or clear it |
| `orbit_start_capture` | Start. Installs the armed hooks. Replaces the previous capture |
| `orbit_stop_capture` | Stop. The events stay until the next start |
| `orbit_capture_status` | Running or not, event count, instrumentation line |
| `orbit_sampling_report` | Hottest sampled functions, one page |
| `orbit_callstacks` | Hottest distinct stacks, outermost frame first |
| `orbit_function_stats` | Count, total, average, min, max, stddev for hooks and manual scopes |

Pages are small on purpose. A raw report or the event ring is not a useful tool result. `offset` walks the next page. Search refuses an offset that would read past 200 hits: narrow the query.

At most 16 functions are armed. That is the service's own cap. Names that exist in more than one module come back as `ambiguous` and are not armed until you pass `module`.

A capture that is already running keeps the hooks it started with. `orbit_unhook` edits the set for the next `orbit_start_capture`.

## Platforms

| | Linux | Windows | macOS |
|---|---|---|---|
| Frida hooks, no rebuild | yes | yes | yes |
| Manual scopes (`orbit_function_stats`) | yes | yes | yes |
| Sampling and call stacks | yes | yes, from an elevated prompt | the tool returns `supported: false` |
| Scheduler and thread states | yes | yes, from an elevated prompt | `supported: false` |
| Kernel uprobes | yes, when you ask, needs root or `CAP_SYS_ADMIN` | refused | refused |
| Auto-profiling | yes, off unless you set it | refused | refused |

Asking to start a capture with sampling on macOS does not start the capture. The result names the unsupported fields. Start again without them. On Windows the capture starts either way; without an elevated prompt the sampling report is empty and the service's `instrumentation` status line says the kernel logger needs one. `orbit_function_stats` is the reading for a Frida hook on every platform. Times are integer nanoseconds.

Sampling can also come back with `"samples": 0` on Linux when `perf_event_paranoid` is too high. That is an empty profile, not an unsupported feature. [Troubleshooting](troubleshooting.md) has the sysctl.

## What is not here

The optimize loop (build, patch, rebuild, statistical gate) lives on the `loop` branch as a Python server. It edits the target's source. It is not part of this server, and `orbit-service mcp` will not apply a patch.
