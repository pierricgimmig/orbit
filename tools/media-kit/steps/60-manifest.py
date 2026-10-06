#!/usr/bin/env python3
"""Write <out>/MANIFEST.md: every output file with size, duration, resolution and caption, the feature
coverage list (features.mjs-free: FEATURES below), and the environment the media was made with.
usage: steps/60-manifest.py --out <dir> [--ref <git-ref> --sha <sha>]"""
import argparse, json, os, subprocess, glob, datetime

FEATURES = [  # (feature, clip ids covering it, note)
    ("Process picker + symbol loading", ["01-attach-record"], ""),
    ("Capture options: CSW, thread states, sampling 1 ms (1 kHz) DWARF, Uprobes hooks", ["01-attach-record"], ""),
    ("Record / live streaming timeline + capture timer", ["01-attach-record", "00-hero"], ""),
    ("Auto-profiling (status line, hooked count, converged) + Functions view", ["03-auto-profile"], ""),
    ("Timeline zoom (Ctrl+wheel, cursor-anchored), drag pan, W/S/A/D", ["02-timeline-navigation"], ""),
    ("Nested dynamic-instrumentation scopes + hover tooltips", ["02-timeline-navigation"], ""),
    ("Scheduler track (one lane per core) + thread focus", ["04-scheduler"], ""),
    ("Thread-state strip", ["04-scheduler"], ""),
    ("Sampling flame graph (hover, click-to-highlight, double-click zoom)", ["05-flame-graph"], ""),
    ("Top-down / bottom-up call trees, expand/collapse all, inclusive/self %", ["06-callstacks"], ""),
    ("Live statistics table + duration histogram", ["07-live-table"], ""),
    ("Time-range selection report (right-drag), per-thread sample-bar selection", ["08-selection-report"], ""),
    ("Service self-profiling lanes (cpu %, rss, ring fill, records lost, events/pass)", ["09-service-health"], ""),
    ("Disassembly + source code view (Source / Disassembly / Both)", ["10-code-view"], ""),
    ("Scope search highlight", ["11-scope-search"], ""),
    ("Colour schemes (7 themes)", ["12-themes"], ""),
    ("Phone-sized (390 px) layout", ["13-mobile-compact"], ""),
    ("Chrome Trace Event JSON import (Open)", ["14-chrome-trace-import"], ""),
]
NOT_CAPTURED = [
    ("Save / Save slice (.orbit.zip) and reopening a slice", "needs a native save dialog; covered by the API (/api/capture/export) not the UI"),
    ("Hook from report (right-click > Hook function) / Functions-tab ticking", "shown only in passing (menu hover in 10-code-view); auto-profile already hooks everything"),
    ("Self pane (F2) and Benchmark (F3)", "developer tools, not a landing-page feature"),
    ("Tracks filter box, track drag-reorder / hide, Inspector, Modules tab, Paper canvas, Compact tracks", "minor UI; not recorded to keep the set short"),
    ("Manual instrumentation API (orbit.h / Rust / Python), orbit-scope CLI, agent track", "needs an instrumented target; game_loop uses dynamic instrumentation only"),
    ("Perfetto (.pftrace) import, static .orbit.stream viewer, website", "not exercised"),
]

def probe(p):
    try:
        j = json.loads(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "stream=width,height:format=duration", "-of", "json", p], capture_output=True, text=True).stdout)
        st = j.get("streams", [{}])[0]; d = j.get("format", {}).get("duration")
        return st.get("width"), st.get("height"), (float(d) if d and d != "N/A" else None)
    except Exception: return None, None, None

# the 3-5 stills recommended for feature cards (name -> what it shows)
STILL_PICKS = {
    "still-timeline-nested-scopes": "live timeline: nested hooked scopes, scheduler lanes, scope tooltip",
    "still-top-down-tree": "top-down call tree, run_frame 88.7% inclusive",
    "still-live-table-histogram": "Live statistics table and simulate_physics duration histogram",
    "still-code-view": "disassembly interleaved with game_loop.c source",
    "still-flame-graph": "sampling flame graph zoomed into dispatch_game_systems",
}

def history(out):
    """generate.sh runs that produced this directory (raw/commands.txt), oldest first."""
    p = os.path.join(out, "raw", "commands.txt")
    if not os.path.exists(p): return []
    lines = [l.strip() for l in open(p) if l.strip()]
    if len(lines) < 2: return []
    return ["Runs that produced this directory (a full run, then partial re-records / re-conversions):", ""] + [f"- `{l}`" for l in lines] + [""]

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--out", required=True); ap.add_argument("--ref", default=""); ap.add_argument("--sha", default="")
    ap.add_argument("--cmd", default="")
    a = ap.parse_args(); out = os.path.abspath(a.out)
    meta = json.load(open(os.path.join(out, "raw", "clips.json")))
    conv = json.load(open(os.path.join(out, "raw", "convert.json"))) if os.path.exists(os.path.join(out, "raw", "convert.json")) else {}
    kit = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    clips = json.loads(subprocess.run(["node", "-e", f"import('{kit}/clips.mjs').then(m=>console.log(JSON.stringify(m.clips)))"], capture_output=True, text=True).stdout)
    cap = {c["id"]: c for c in clips}
    cap["00-hero"] = {"title": "Hero montage", "caption": "Montage of the best moments: " + ", ".join(s[0].replace(".mkv", "") for s in conv.get("00-hero", {}).get("segments", []))}
    L = [f"# Orbit live viewer media", "",
         f"Generated {datetime.datetime.now().strftime('%Y-%m-%d %H:%M %Z')} by orbit-media-kit" + (f" from `{a.ref}` ({a.sha})" if a.ref else "") + ".",
         (f"Command: `{a.cmd}`" if a.cmd else ""), "",
         *history(out),
         "Recorded live from orbit-service in a Debian 13 VM (QEMU TCG, 6 vCPU, 2 GB) capturing the `game_loop` dummy, viewer in headful Chrome "
         "(SwiftShader WebGL) at 1600x1000 device px (1280x800 CSS px, DPR 1.25). Software GL renders the viewer at roughly 8-12 fps, so the "
         "videos carry that frame rate (encoded at 30 fps); clips longer than ~15 s play faster, up to 1.75x (speed column).", "",
         "| File | Size | Duration | Resolution | Speed | Caption |", "|---|---:|---:|---|---:|---|"]
    ids = ["00-hero"] + [c["id"] for c in clips if c["id"] in meta]
    for cid in ids:
        for ext in ("mp4", "webm", "gif", "png"):
            p = os.path.join(out, f"{cid}.{ext}")
            if not os.path.exists(p): continue
            w, h, d = probe(p); sz = os.path.getsize(p)
            szs = f"{sz/1e6:.2f} MB" if sz > 1e5 else f"{sz/1e3:.0f} kB"
            sp = conv.get(cid, {}).get("speed", "")
            c = cap.get(cid, {}); text = f"**{c.get('title','')}** - {c.get('caption','')}" if ext == "mp4" else ("poster frame" if ext == "png" else "")
            L.append(f"| `{cid}.{ext}` | {szs} | {f'{d:.1f} s' if d and ext != 'png' else ''} | {w}x{h} | {f'{sp}x' if sp and ext != 'png' else ''} | {text} |")
    st = sorted(glob.glob(os.path.join(out, "stills", "*.png")))
    if st:
        L += ["", "## Stills (full resolution, for feature cards)", "", "Picks for the landing page are marked with a star.", "",
              "| File | Size | Resolution | Pick |", "|---|---:|---|---|"]
        for p in st:
            w, h, _ = probe(p); n = os.path.basename(p)[:-4]
            L.append(f"| `stills/{n}.png` | {os.path.getsize(p)/1e3:.0f} kB | {w}x{h} | {'★ ' + STILL_PICKS[n] if n in STILL_PICKS else ''} |")
    L += ["", "## Feature coverage", "", "| Feature | Clip(s) |", "|---|---|"]
    for f, cs, note in FEATURES:
        ok = [c for c in cs if c in meta or c == "00-hero"]
        L.append(f"| {f} | {', '.join(ok) if ok else 'not recorded'}{(' - ' + note) if note else ''} |")
    L += ["", "### Not captured", ""] + [f"- {f}: {why}" for f, why in NOT_CAPTURED]
    errs = [(k, v["error"]) for k, v in meta.items() if v.get("error")]
    if errs: L += ["", "### Scene errors", ""] + [f"- {k}: `{e.splitlines()[0]}`" for k, e in errs]
    open(os.path.join(out, "MANIFEST.md"), "w").write("\n".join(L) + "\n")
    print("wrote", os.path.join(out, "MANIFEST.md"))

main()
