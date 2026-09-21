#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Builds the *baseline post* for an optimization target: one self-contained
HTML file that carries the real Orbit capture (the `stream` export, gzipped
and base64-embedded), the sampling hotspot tables, the hooked-scope table,
and the hand-written overview -- how the program works, where the time
goes, which wins are on the table -- rendered from Markdown.

    python3 tools/optimize-loop/baseline_post.py \
        --capture-json base.capture.json --report-json base.report.json \
        --stream base.orbit.stream [--bundle base.orbit.zip] \
        --overview overview.md --title "Stockfish under Orbit" \
        --out docs/blog/NN-slug.html

The page needs nothing but a viewer next to it: "Open in Orbit" hands the
embedded bytes to `../viewer/index.html?capture=<blob url>` (the site layout,
where blog/ and viewer/ are siblings), "Show here" does the same in an
iframe, and "Download" saves the `.orbit.stream` for any other viewer.
Standard library only; the hooked-scope table needs pyarrow when a bundle is
given (otherwise pass --hooks-json, or it is omitted).
"""
import argparse
import base64
import datetime
import gzip
import html
import io
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(REPO, "tools", "site"))
from build_site import render_markdown  # noqa: E402

CSS = """
:root{--ground:#f1f4f0;--surface:#fafbf9;--surface-2:#e7ece6;--ink:#131a17;--ink-soft:#3c4741;--muted:#62716a;
--line:#d3dbd3;--line-strong:#b7c3ba;--accent:#1F7B95;--accent-deep:#155E72;--accent-wash:#E1EEF2;--warn:#8a6212;--warn-wash:#f4ecda;
--display:"Archivo","Helvetica Neue",Arial,sans-serif;--body:"Newsreader",Georgia,"Times New Roman",serif;--mono:"IBM Plex Mono",ui-monospace,Menlo,monospace;
--measure:40rem;--wide:64rem}
:root:not([data-theme="light"]){color-scheme:light dark}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--ground:#0f1412;--surface:#161d1a;--surface-2:#1d2622;--ink:#e7ede9;--ink-soft:#c3cdc8;--muted:#8fa098;--line:#29332e;--line-strong:#3b4842;--accent:#58B4CE;--accent-deep:#7CC9DE;--accent-wash:#123039;--warn:#d9ae5a;--warn-wash:#2a2417}}
:root[data-theme="dark"]{--ground:#0f1412;--surface:#161d1a;--surface-2:#1d2622;--ink:#e7ede9;--ink-soft:#c3cdc8;--muted:#8fa098;--line:#29332e;--line-strong:#3b4842;--accent:#58B4CE;--accent-deep:#7CC9DE;--accent-wash:#123039;--warn:#d9ae5a;--warn-wash:#2a2417}
*{box-sizing:border-box}body{margin:0;background:var(--ground);color:var(--ink);font-family:var(--body);font-size:1.0625rem;line-height:1.62;-webkit-font-smoothing:antialiased}
.masthead{border-bottom:1px solid var(--line);background:var(--surface)}.masthead-inner{max-width:var(--wide);margin:0 auto;padding:3rem 1.25rem 2rem;display:flex;flex-direction:column;gap:1rem}
.eyebrow{font-family:var(--mono);font-size:.72rem;letter-spacing:.13em;text-transform:uppercase;color:var(--accent);margin:0}
h1{font-family:var(--display);font-weight:700;font-size:clamp(2.2rem,6vw,3.6rem);line-height:1.02;letter-spacing:-.02em;margin:0;max-width:18ch}
.dek{font-size:1.2rem;line-height:1.5;color:var(--ink-soft);max-width:var(--measure);margin:0}
.byline{font-family:var(--mono);font-size:.78rem;color:var(--muted);margin:0;max-width:var(--wide)}
.doc{max-width:var(--wide);margin:0 auto;padding:1.5rem 1.25rem 4rem}
.doc p,.doc ul,.doc ol{max-width:var(--measure)}
h2{font-family:var(--display);font-weight:600;font-size:1.55rem;letter-spacing:-.01em;margin:2.6rem 0 .8rem;padding-top:1.2rem;border-top:1px solid var(--line)}
h3{font-family:var(--display);font-weight:600;font-size:1.15rem;margin:1.6rem 0 .5rem}
code,pre,kbd{font-family:var(--mono);font-size:.86em}code{background:var(--surface-2);padding:.08em .35em;border-radius:4px}
pre{background:var(--surface-2);border:1px solid var(--line);border-radius:8px;padding:.9rem 1rem;overflow-x:auto;max-width:100%}pre code{background:none;padding:0}
a{color:var(--accent-deep)}
.table-wrap{overflow-x:auto;margin:1rem 0 1.4rem}table{border-collapse:collapse;font-size:.92rem;width:100%}
th,td{padding:.42rem .7rem;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}th{font-family:var(--display);font-size:.8rem;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);font-weight:600}
td.num,th.num{text-align:right;font-family:var(--mono);white-space:nowrap}
.bar{display:inline-block;height:.6em;background:var(--accent);vertical-align:middle;border-radius:2px;margin-right:.4em}
.capture{background:var(--surface);border:1px solid var(--line);border-radius:12px;padding:1.1rem 1.3rem;margin:1.4rem 0}
.capture .facts{font-family:var(--mono);font-size:.8rem;color:var(--muted);margin:.4rem 0 .9rem}
.capture button,.capture a.btn{font-family:var(--display);font-weight:600;font-size:.92rem;background:var(--accent);color:#fff;border:0;border-radius:8px;padding:.55rem 1rem;cursor:pointer;text-decoration:none;margin-right:.5rem}
.capture button.secondary,.capture a.btn.secondary{background:var(--surface-2);color:var(--ink);border:1px solid var(--line-strong)}
.capture .status{font-family:var(--mono);font-size:.8rem;color:var(--muted);margin-top:.7rem;min-height:1.2em}
.capture iframe{width:100%;height:78vh;border:1px solid var(--line);border-radius:8px;margin-top:1rem;background:var(--surface-2);display:none}
.note{border-left:3px solid var(--accent);background:var(--accent-wash);padding:.8rem 1rem;border-radius:0 8px 8px 0;margin:1.2rem 0;max-width:var(--measure)}
.note.warn{border-left-color:var(--warn);background:var(--warn-wash)}
.orbit-theme-toggle{position:fixed;bottom:1rem;right:1rem;z-index:50;font-family:var(--mono);font-size:.72rem;color:var(--muted);background:var(--surface);border:1px solid var(--line);border-radius:999px;padding:.35rem .7rem;cursor:pointer}
footer{border-top:1px solid var(--line);font-family:var(--mono);font-size:.78rem;color:var(--muted);padding:1.5rem 1.25rem;max-width:var(--wide);margin:0 auto}
"""

JS = r"""
(function(){
  var b=document.getElementById("orbit-theme-toggle");
  function cur(){return document.documentElement.getAttribute("data-theme")||"light";}
  function lab(){b.textContent=cur()==="dark"?"Light":"Dark";}
  lab();b.addEventListener("click",function(){var n=cur()==="dark"?"light":"dark";document.documentElement.setAttribute("data-theme",n);try{localStorage.setItem("orbit-theme",n);}catch(e){}lab();});
})();
// The capture: gzip-compressed stream export, base64 in a <script> block.
// Decoded once, on demand, into a same-origin blob URL the viewer fetches.
var orbitCapture=(function(){
  var blobUrl=null;
  function status(t){document.getElementById("capture-status").textContent=t;}
  async function bytes(){
    var b64=document.getElementById("orbit-capture-data").textContent.replace(/\s+/g,"");
    var bin=atob(b64), gz=new Uint8Array(bin.length);
    for(var i=0;i<bin.length;i++)gz[i]=bin.charCodeAt(i);
    if(typeof DecompressionStream==="undefined")throw new Error("this browser cannot inflate gzip (no DecompressionStream)");
    var ds=new DecompressionStream("gzip");
    var stream=new Blob([gz]).stream().pipeThrough(ds);
    return new Uint8Array(await new Response(stream).arrayBuffer());
  }
  async function url(){
    if(blobUrl)return blobUrl;
    status("inflating capture…");
    var data=await bytes();
    blobUrl=URL.createObjectURL(new Blob([data],{type:"application/octet-stream"}));
    status((data.length/1048576).toFixed(1)+" MB stream ready");
    return blobUrl;
  }
  function viewer(){
    var v=document.getElementById("orbit-viewer-url").value||"../viewer/index.html";
    return new URL(v,location.href).href;
  }
  async function open(){
    try{var u=await url();window.open(viewer()+"?capture="+encodeURIComponent(u)+"&collapse=scheduler","_blank");}
    catch(e){status("could not open: "+e.message);}
  }
  async function inline(){
    try{var u=await url();var f=document.getElementById("orbit-viewer-frame");
      f.src=viewer()+"?capture="+encodeURIComponent(u)+"&collapse=scheduler";f.style.display="block";f.scrollIntoView({behavior:"smooth"});}
    catch(e){status("could not show: "+e.message);}
  }
  async function download(){
    try{var data=await bytes();var a=document.createElement("a");a.href=URL.createObjectURL(new Blob([data]));
      a.download=document.getElementById("orbit-capture-name").textContent;a.click();}
    catch(e){status("could not download: "+e.message);}
  }
  return {open:open,inline:inline,download:download,url:url};
})();
"""


def esc(s):
    return html.escape(str(s), quote=True)


def hotspot_tables(report, limit):
    fns = report.get("functions") or []
    total = report.get("samples") or 0
    by_self = sorted(fns, key=lambda f: -(f.get("self_percent") or 0))[:limit]
    by_incl = sorted(fns, key=lambda f: -(f.get("inclusive_percent") or 0))[:limit]

    def table(rows, key, label):
        out = [f'<div class="table-wrap"><table><thead><tr><th class="num">{label} %</th><th class="num">samples</th><th>function</th></tr></thead><tbody>']
        for f in rows:
            pct = f.get(key) or 0
            cnt = f.get("self" if key == "self_percent" else "inclusive") or 0
            out.append(f'<tr><td class="num"><span class="bar" style="width:{min(pct, 100) * 2.2:.0f}px"></span>{pct:.2f}</td>'
                       f'<td class="num">{cnt:,}</td><td><code>{esc(demangle(f.get("name", "")))}</code></td></tr>')
        out.append("</tbody></table></div>")
        return "".join(out)

    return (f"<p>{total:,} callstack samples. Self time is where the instruction pointer was; inclusive counts every "
            f"sample with the function anywhere on the stack.</p>"
            f"<h3>Top by self time</h3>{table(by_self, 'self_percent', 'self')}"
            f"<h3>Top by inclusive time</h3>{table(by_incl, 'inclusive_percent', 'incl')}")


_DEMANGLE_CACHE = {}


def demangle(name):
    """c++filt if present, trimmed to the qualified name (no parameter list)."""
    if not name.startswith("_Z"):
        return name
    if name in _DEMANGLE_CACHE:
        return _DEMANGLE_CACHE[name]
    try:
        out = subprocess.run(["c++filt", name], capture_output=True, text=True, timeout=5).stdout.strip() or name
    except Exception:
        out = name
    # Keep "ns::Class::fn<T>" and drop the parameter list, which is noise in
    # a table. Template arguments can hold parentheses ("<(NodeType)0>"), so
    # the list is found from the end: the "(" matching the final ")".
    body = out.split(" [clone ")[0]
    body = body[:-6] if body.endswith(" const") else body
    if body.endswith(")"):
        depth = 0
        for i in range(len(body) - 1, -1, -1):
            depth += body[i] == ")"
            depth -= body[i] == "("
            if depth == 0:
                body = body[:i]
                break
    # A leading return type ("int ns::f<T>") is noise too; the name is the
    # last space-separated token that carries the scope operator.
    if " " in body and "::" in body and "(" not in body.split("::")[0]:
        head, _, tail = body.rpartition(" ")
        if "::" in tail and "<" not in head:
            body = tail
    out = body or out
    _DEMANGLE_CACHE[name] = out
    return out


def hooks_table(bundle, hooks_json, capture):
    rows = None
    if hooks_json:
        rows = json.load(open(hooks_json))
    elif bundle:
        try:
            import zipfile
            import pyarrow.parquet as pq
            z = zipfile.ZipFile(bundle)
            pid = json.loads(z.read("manifest.json"))["bundle"].get("target_pid")
            ev = pq.read_table(io.BytesIO(z.read("events.parquet"))).to_pandas()
            api = ev[(ev.kind == 1) & (ev.pid == pid)]
            g = api.groupby("name").duration_ns.agg(["count", "mean", "min", "max", "sum"])
            rows = [{"name": n, "calls": int(r["count"]), "mean_ms": r["mean"] / 1e6, "min_ms": r["min"] / 1e6,
                     "max_ms": r["max"] / 1e6, "total_s": r["sum"] / 1e9} for n, r in g.iterrows()]
            rows.sort(key=lambda r: r["calls"])
        except Exception as error:  # pyarrow missing, or an older bundle
            return f'<p class="note warn">Hooked-scope table skipped: {esc(error)}</p>'
    if not rows:
        return ""
    out = ['<div class="table-wrap"><table><thead><tr><th>hooked function</th><th class="num">calls</th><th class="num">mean ms</th>'
           '<th class="num">min ms</th><th class="num">max ms</th><th class="num">total s</th></tr></thead><tbody>']
    for r in rows:
        out.append(f'<tr><td><code>{esc(demangle(r["name"]))}</code></td><td class="num">{r["calls"]:,}</td>'
                   f'<td class="num">{r["mean_ms"]:.2f}</td><td class="num">{r["min_ms"]:.2f}</td>'
                   f'<td class="num">{r["max_ms"]:.2f}</td><td class="num">{r["total_s"]:.2f}</td></tr>')
    out.append("</tbody></table></div>")
    status = (capture.get("status") or {}).get("instrumentation", "")
    if status:
        out.append(f'<p class="byline">service status at stop: {esc(status)}</p>')
    return "".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--capture-json", required=True, help="baseline_capture.py's .capture.json")
    ap.add_argument("--report-json", required=True, help="the sampling report (.report.json)")
    ap.add_argument("--stream", required=True, help="the .orbit.stream export to embed")
    ap.add_argument("--bundle", help="the .orbit.zip export, for the hooked-scope table (needs pyarrow)")
    ap.add_argument("--hooks-json", help="precomputed hooked-scope rows instead of --bundle")
    ap.add_argument("--overview", required=True, help="Markdown: how the program works, hotspots read, candidate wins")
    ap.add_argument("--title", required=True)
    ap.add_argument("--dek", default="", help="one-paragraph summary under the title")
    ap.add_argument("--eyebrow", default="Orbit optimize loop · baseline")
    ap.add_argument("--target", default="", help="what was profiled: repo, commit, build")
    ap.add_argument("--limit", type=int, default=25, help="rows per hotspot table")
    ap.add_argument("--viewer", default="../viewer/index.html", help="viewer URL relative to the post")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    capture = json.load(open(args.capture_json))
    report = json.load(open(args.report_json))
    raw = open(args.stream, "rb").read()
    gz = gzip.compress(raw, compresslevel=9)
    b64 = base64.b64encode(gz).decode()
    b64 = "\n".join(b64[i:i + 120] for i in range(0, len(b64), 120))
    overview = render_markdown(open(args.overview).read())
    name = os.path.basename(args.stream)
    when = datetime.datetime.now().strftime("%Y-%m-%d")
    cmd = " ".join([os.path.basename(capture.get("binary", "")), *capture.get("args", [])])
    facts = (f"{capture.get('samples', report.get('samples', 0)):,} samples · {len(capture.get('hooks') or [])} hooks · "
             f"{capture.get('wall_s', '?')} s · stream {len(raw) / 1048576:.1f} MB ({len(gz) / 1048576:.1f} MB embedded) · "
             f"<code>{esc(cmd)}</code>")

    page = f"""<!doctype html>
<html lang="en">
<head>
<script>try{{var t=localStorage.getItem("orbit-theme");if(t)document.documentElement.setAttribute("data-theme",t);}}catch(e){{}}</script>
<meta charset="utf-8">
<title>{esc(args.title)} — Orbit</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Archivo:wght@500;600;700&family=IBM+Plex+Mono:wght@400;500&family=Newsreader:opsz,wght@6..72,400;6..72,500;6..72,600&display=swap">
<style>{CSS}</style>
</head>
<body>
<header class="masthead"><div class="masthead-inner">
<p class="eyebrow">{esc(args.eyebrow)}</p>
<h1>{esc(args.title)}</h1>
{f'<p class="dek">{args.dek}</p>' if args.dek else ''}
<p class="byline">Baseline taken {when}. {esc(args.target)}</p>
</div></header>
<article class="doc">

<h2 id="capture">The capture</h2>
<div class="capture">
<p style="margin:0"><strong>The real Orbit capture is in this file.</strong> Open it in the viewer next to this page, show it inline, or download the stream.</p>
<p class="facts">{facts}</p>
<button type="button" onclick="orbitCapture.open()">Open in Orbit</button>
<button type="button" class="secondary" onclick="orbitCapture.inline()">Show here</button>
<button type="button" class="secondary" onclick="orbitCapture.download()">Download <span id="orbit-capture-name">{esc(name)}</span></button>
<label class="byline" style="display:inline-block;margin-left:.6rem">viewer: <input id="orbit-viewer-url" value="{esc(args.viewer)}" size="26" style="font-family:var(--mono);font-size:.8rem"></label>
<div class="status" id="capture-status"></div>
<iframe id="orbit-viewer-frame" title="Orbit viewer"></iframe>
</div>

<h2 id="hooks">Structure: the hooked scopes</h2>
{hooks_table(args.bundle, args.hooks_json, capture)}

<h2 id="hotspots">Hotspots: the sampling report</h2>
{hotspot_tables(report, args.limit)}

{overview}
</article>
<footer>Generated by <code>tools/optimize-loop/baseline_post.py</code> · capture <code>{esc(name)}</code> embedded as gzip+base64 · <a href="index.html">All posts</a></footer>
<button class="orbit-theme-toggle" id="orbit-theme-toggle" type="button" aria-label="Toggle colour theme">Dark</button>
<script>{JS}</script>
<script type="application/gzip-base64" id="orbit-capture-data">
{b64}
</script>
</body></html>
"""
    open(args.out, "w").write(page)
    print(f"wrote {args.out}: {os.path.getsize(args.out) / 1048576:.1f} MB "
          f"(stream {len(raw) / 1048576:.1f} MB → gzip {len(gz) / 1048576:.1f} MB)")


if __name__ == "__main__":
    main()
