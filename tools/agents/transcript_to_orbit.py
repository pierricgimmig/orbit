#!/usr/bin/env python3
"""Follow a coding agent's own transcript and draw it on Orbit's timeline.

Claude Code writes every session to ~/.claude/projects/<cwd>/<session>.jsonl:
one JSON record per line, timestamped, with the assistant's tool calls
(`tool_use`, with the tool's name and input), the results (`tool_result`,
paired by id), the text it says, the user's prompts, token usage, and
`isSidechain` on everything a sub-agent does. Nothing has to be added to
the agent for this to exist; it is what the agent already leaves behind.

This script tails one transcript (the newest by default) and posts it to a
running orbit-service as a batch every second:

    process   the session: "claude <dir> · <id>"
    thread    "agent" for the main conversation; one thread per sub-agent
    span      every tool call, from the call to its result: "Bash: <description>"
    span      "model": the time between a result and the next assistant message,
              which is the model generating (there is nothing else in that gap)
    instant   "prompt: …" when the user speaks, "says: …" when the agent does,
              "running: …" the moment a tool call starts (before its result)
    value     "output tokens" and "context tokens" per assistant message

    python3 tools/agents/transcript_to_orbit.py                 # newest session, follow
    python3 tools/agents/transcript_to_orbit.py --file X.jsonl --once --summary
    ORBIT_URL=http://host:44766 python3 tools/agents/transcript_to_orbit.py --all

Only names, timings and counts leave the machine: a tool call is named by
the description the agent gave it (or the first 60 characters of the
command), never by its output. Needs Orbit with POST /api/events (#87).
"""
import argparse, glob, json, os, sys, time, zlib, urllib.request
from collections import defaultdict
from datetime import datetime, timezone

MAIN_TID = 1
NAME_CHARS = 60


def unix_ns(iso):
    return int(datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1e9)


def short(text, n=NAME_CHARS):
    text = " ".join(str(text).split())
    return text if len(text) <= n else text[: n - 1] + "…"


def tool_name(name, inp):
    inp = inp or {}
    if name == "Bash":
        return "Bash: " + short(inp.get("description") or inp.get("command", ""))
    if name in ("Read", "Edit", "Write", "NotebookEdit"):
        return f"{name} {os.path.basename(str(inp.get('file_path', '')))}"
    if name in ("Agent", "Task"):
        return "agent: " + short(inp.get("description") or inp.get("prompt", ""))
    if name == "Skill":
        return f"Skill /{inp.get('skill', '')}"
    if name in ("Grep", "Glob"):
        return f"{name} {short(inp.get('pattern', ''), 40)}"
    return name


class Session:
    """One transcript's state: open tool calls, sub-agent threads, the last
    time the model was handed something to think about."""

    def __init__(self, path):
        self.path = path
        self.offset = 0
        self.partial = ""
        self.pid = None
        self.session_id = None
        self.named = False
        self.cwd = None
        self.open_tools = {}      # tool_use id -> (tid, name, start_ns, depth)
        self.depth = defaultdict(int)   # tid -> open spans
        self.side_root = {}       # uuid -> tid, for sidechain records
        self.side_tids = {}       # root uuid -> tid
        self.next_side_tid = MAIN_TID + 1
        self.model_since = {}     # tid -> ns when the model last got input
        self.stats = defaultdict(float)
        self.counts = defaultdict(int)

    def tid_for(self, rec):
        if not rec.get("isSidechain"):
            return MAIN_TID
        uuid, parent = rec.get("uuid"), rec.get("parentUuid")
        if parent in self.side_root:
            tid = self.side_root[parent]
        else:
            tid = self.next_side_tid
            self.next_side_tid += 1
            self.side_tids[uuid] = tid
        if uuid:
            self.side_root[uuid] = tid
        return tid

    def read_new(self):
        """New complete lines since the last read, as parsed records."""
        try:
            size = os.path.getsize(self.path)
        except OSError:
            return []
        if size < self.offset:            # truncated / rotated
            self.offset, self.partial = 0, ""
        if size == self.offset:
            return []
        with open(self.path, "rb") as f:
            f.seek(self.offset)
            chunk = f.read(size - self.offset)
        self.offset = size
        text = self.partial + chunk.decode("utf-8", "replace")
        lines = text.split("\n")
        self.partial = lines.pop()        # "" when the chunk ended in a newline
        out = []
        for line in lines:
            if not line.strip():
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return out

    def batch(self, records):
        """Turn records into an /api/events body (without the clock)."""
        b = {"processes": [], "threads": [], "spans": [], "instants": [], "values": []}
        for rec in records:
            ts = rec.get("timestamp")
            msg = rec.get("message")
            kind = rec.get("type")
            if not ts or not isinstance(msg, dict) or kind not in ("assistant", "user"):
                continue
            if self.pid is None:
                self.session_id = rec.get("sessionId") or os.path.basename(self.path).split(".")[0]
                self.cwd = rec.get("cwd") or ""
                self.pid = 0x10000 + (zlib.crc32(self.session_id.encode()) % 0x7FFF0000)
                label = f"claude {os.path.basename(self.cwd.rstrip('/')) or '?'} · {self.session_id[:8]}"
                b["processes"].append({"pid": self.pid, "name": label})
                b["threads"].append({"pid": self.pid, "tid": MAIN_TID, "name": "agent"})
            t = unix_ns(ts)
            tid = self.tid_for(rec)
            if tid != MAIN_TID and tid not in [th["tid"] for th in b["threads"]] and tid not in self.__dict__.setdefault("_named_tids", set()):
                self._named_tids.add(tid)
                b["threads"].append({"pid": self.pid, "tid": tid, "name": f"sub-agent {tid - MAIN_TID}"})
            content = msg.get("content")
            if isinstance(content, str):
                content = [{"type": "text", "text": content}]
            content = content or []
            if kind == "assistant":
                # The gap since the model was last given something is the
                # model's own time: generating this message.
                since = self.model_since.get(tid)
                if since and t > since:
                    b["spans"].append({"pid": self.pid, "tid": tid, "name": "model", "start_ns": since,
                                       "duration_ns": t - since, "depth": self.depth[tid], "track": "scope"})
                    self.stats["model_s"] += (t - since) / 1e9
                self.model_since[tid] = None
                usage = msg.get("usage") or {}
                if usage.get("output_tokens") is not None:
                    b["values"].append({"pid": self.pid, "tid": tid, "name": "output tokens", "timestamp_ns": t,
                                        "value": float(usage.get("output_tokens", 0))})
                    ctx = sum(usage.get(k, 0) or 0 for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
                    b["values"].append({"pid": self.pid, "tid": tid, "name": "context tokens", "timestamp_ns": t, "value": float(ctx)})
                for c in content:
                    if not isinstance(c, dict):
                        continue
                    if c.get("type") == "tool_use":
                        name = tool_name(c.get("name", "tool"), c.get("input"))
                        self.open_tools[c.get("id")] = (tid, name, t, self.depth[tid])
                        self.depth[tid] += 1
                        b["instants"].append({"pid": self.pid, "tid": tid, "name": "running: " + name, "timestamp_ns": t, "depth": 0})
                    elif c.get("type") == "text" and c.get("text", "").strip():
                        b["instants"].append({"pid": self.pid, "tid": tid, "name": "says: " + short(c["text"]), "timestamp_ns": t, "depth": 0})
                        self.counts["replies"] += 1
            else:  # user: prompts and tool results
                for c in content:
                    if not isinstance(c, dict):
                        continue
                    if c.get("type") == "tool_result" and c.get("tool_use_id") in self.open_tools:
                        otid, name, start, depth = self.open_tools.pop(c["tool_use_id"])
                        self.depth[otid] = max(0, self.depth[otid] - 1)
                        dur = max(t - start, 1)
                        b["spans"].append({"pid": self.pid, "tid": otid, "name": name, "start_ns": start,
                                           "duration_ns": dur, "depth": depth, "track": "scope"})
                        tool = name.split(":")[0].split(" ")[0]
                        self.stats["tool:" + tool] += dur / 1e9
                        self.counts["tool:" + tool] += 1
                        self.model_since[otid] = t
                    elif c.get("type") == "text" and c["text"].strip() and not rec.get("isMeta"):
                        b["instants"].append({"pid": self.pid, "tid": tid, "name": "prompt: " + short(c["text"]), "timestamp_ns": t, "depth": 0})
                        self.counts["prompts"] += 1
                        self.model_since[tid] = t
        return b


def post(url, body):
    req = urllib.request.Request(url + "/api/events", data=json.dumps(body).encode(), method="POST",
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--file", action="append", help="a transcript to follow (repeatable); default: the newest")
    ap.add_argument("--all", action="store_true", help="follow every transcript changed in the last hour, and new ones")
    ap.add_argument("--url", default=os.environ.get("ORBIT_URL", "http://127.0.0.1:44766"))
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("--once", action="store_true", help="replay what is there and exit")
    ap.add_argument("--since", default=None, help="replay only records newer than this age (45m, 2h, 1d); default: everything")
    ap.add_argument("--summary", action="store_true", help="print where the time went, per session")
    ap.add_argument("--dry-run", action="store_true", help="do not post; just count")
    args = ap.parse_args()
    root = os.path.expanduser("~/.claude/projects")
    since_ns = None
    if args.since:
        unit = {"s": 1, "m": 60, "h": 3600, "d": 86400}[args.since[-1]]
        since_ns = int((time.time() - float(args.since[:-1]) * unit) * 1e9)

    def discover():
        files = glob.glob(os.path.join(root, "*", "*.jsonl"))
        if args.all:
            cutoff = time.time() - 3600
            return [f for f in files if os.path.getmtime(f) > cutoff]
        return sorted(files, key=os.path.getmtime)[-1:]

    paths = args.file or discover()
    sessions = {p: Session(p) for p in paths}
    if not sessions:
        sys.exit("no transcript found")
    while True:
        if args.all:
            for p in discover():
                sessions.setdefault(p, Session(p))
        for s in list(sessions.values()):
            recs = s.read_new()
            if since_ns:
                recs = [r for r in recs if not r.get("timestamp") or unix_ns(r["timestamp"]) >= since_ns]
            if not recs:
                continue
            body = s.batch(recs)
            n = sum(len(body[k]) for k in ("spans", "instants", "values"))
            if n == 0 and not body["processes"]:
                continue
            body["clock"] = "unix_ns"
            if args.dry_run:
                print(f"{os.path.basename(s.path)[:8]}: {n} records (dry run)", file=sys.stderr)
                continue
            try:
                r = post(args.url, body)
                print(f"{os.path.basename(s.path)[:8]}: {n} records, accepted {r.get('accepted')}, dropped {r.get('dropped_before_start')}", file=sys.stderr)
            except Exception as e:  # noqa: BLE001 - keep following through an outage
                print(f"orbit: {e}", file=sys.stderr)
        if args.once:
            break
        time.sleep(args.interval)
    if args.summary:
        for s in sessions.values():
            if s.pid is None:
                continue
            print(f"\n{os.path.basename(s.path)[:8]}  {s.cwd}")
            tools = sorted(((v, k) for k, v in s.stats.items() if k.startswith("tool:")), reverse=True)
            print(f"  model      {s.stats['model_s']:8.1f} s   ({s.counts['replies']} replies, {s.counts['prompts']} prompts)")
            for secs, k in tools:
                print(f"  {k[5:]:<10} {secs:8.1f} s   ({s.counts[k]} calls)")
            if s.open_tools:
                print(f"  still running: {', '.join(v[1] for v in s.open_tools.values())}")


if __name__ == "__main__":
    main()
