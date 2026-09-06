#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Builds the project web site: a static directory with the viewer, one
capture it opens on the front page with no service behind it, the manual,
the blog and the screenshots.

    python3 tools/site/build_site.py                      # captures Box3D for the front page
    python3 tools/site/build_site.py --stream my.orbit.stream
    python3 tools/site/build_site.py --bundle capture.orbit.zip
    python3 tools/site/serve.py --dir site --port 8081    # then open it

The front-page capture is a `.orbit.stream`: the wire frames a connecting
viewer receives, saved as one file (`GET /api/capture/export?format=stream`).
The viewer opens it with `?capture=<url>` and never talks to a service, so
the site is plain files on any host. Standard library only, like the e2e
suite: no pip.
"""

import argparse
import html
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(REPO, "tools", "e2e"))

VIEWER_DIST = os.path.join(REPO, "src/OrbitLiveViewer/viewer-dist")


# ------------------------------------------------------------------ markdown


def render_markdown(text):
    """The subset of Markdown the docs use: headings, paragraphs, fenced code,
    lists, tables, links, images, bold and inline code."""
    out = []
    lines = text.splitlines()
    i = 0
    in_list = None
    para = []

    def flush_para():
        if para:
            out.append(f"<p>{inline(' '.join(para))}</p>")
            para.clear()

    def close_list():
        nonlocal in_list
        if in_list:
            out.append(f"</{in_list}>")
            in_list = None

    while i < len(lines):
        line = lines[i]
        if line.startswith("```"):
            flush_para()
            close_list()
            lang = line[3:].strip()
            block = []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                block.append(lines[i])
                i += 1
            out.append(f'<pre><code class="{html.escape(lang)}">{html.escape(chr(10).join(block))}</code></pre>')
            i += 1
            continue
        m = re.match(r"^(#{1,6})\s+(.*)$", line)
        if m:
            flush_para()
            close_list()
            level = len(m.group(1))
            out.append(f"<h{level} id=\"{slug(m.group(2))}\">{inline(m.group(2))}</h{level}>")
            i += 1
            continue
        if line.startswith("|") and i + 1 < len(lines) and re.match(r"^\|[\s\-:|]+\|$", lines[i + 1]):
            flush_para()
            close_list()
            header = [c.strip() for c in line.strip("|").split("|")]
            out.append("<table><thead><tr>" + "".join(f"<th>{inline(c)}</th>" for c in header) + "</tr></thead><tbody>")
            i += 2
            while i < len(lines) and lines[i].startswith("|"):
                cells = [c.strip() for c in lines[i].strip("|").split("|")]
                out.append("<tr>" + "".join(f"<td>{inline(c)}</td>" for c in cells) + "</tr>")
                i += 1
            out.append("</tbody></table>")
            continue
        m = re.match(r"^(\s*)([-*]|\d+\.)\s+(.*)$", line)
        if m:
            flush_para()
            kind = "ol" if m.group(2)[0].isdigit() else "ul"
            if in_list != kind:
                close_list()
                out.append(f"<{kind}>")
                in_list = kind
            item = [m.group(3)]
            i += 1
            # continuation lines of the same item are indented
            while i < len(lines) and lines[i].startswith("  ") and not re.match(r"^\s*([-*]|\d+\.)\s+", lines[i]):
                item.append(lines[i].strip())
                i += 1
            out.append(f"<li>{inline(' '.join(item))}</li>")
            continue
        if not line.strip():
            flush_para()
            close_list()
            i += 1
            continue
        para.append(line.strip())
        i += 1
    flush_para()
    close_list()
    return "\n".join(out)


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def inline(text):
    text = html.escape(text, quote=False)
    text = re.sub(r"!\[([^\]]*)\]\(([^)]+)\)", r'<img alt="\1" src="\2">', text)
    text = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', text)
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    text = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", text)
    return text


def page(title, body, root=".", nav=True):
    template = open(os.path.join(HERE, "page.html")).read()
    return (template.replace("{{title}}", html.escape(title))
            .replace("{{root}}", root)
            .replace("{{body}}", body)
            .replace("{{nav}}", NAV.replace("{{root}}", root) if nav else ""))


NAV = ('<nav class="site"><a class="brand" href="{{root}}/index.html"><img src="{{root}}/logo.png" alt="Orbit"></a>'
       '<a href="{{root}}/manual/index.html">Manual</a>'
       '<a href="{{root}}/blog/index.html">Blog</a>'
       '<a href="{{root}}/e2e/report.html">Test report</a>'
       '<span class="spacer"></span>'
       '<a class="cta" href="{{root}}/viewer/index.html?capture=../captures/{{capture}}&collapse=scheduler">Open the viewer</a></nav>')


# ------------------------------------------------------------------- capture


def capture_stream(bundle, port):
    """Produces a stream file from a bundle, or from a fresh Box3D capture."""
    from orbit_e2e import DEFAULT_BOX3D, Service, Target, build_target  # noqa: E402

    service = Service(port)
    target = None
    try:
        if bundle:
            reply = service.post("/api/capture/open", {"path": os.path.abspath(bundle)})
            if reply.startswith("HTTP"):
                raise SystemExit(f"could not open {bundle}: {reply}")
            deadline = time.time() + 30
            while time.time() < deadline and service.get("/api/status")["events_live"] == 0:
                time.sleep(0.2)
        else:
            target_bin = "/tmp/orbit-site-box3d-target"
            build_target(DEFAULT_BOX3D, target_bin)
            target = Target([target_bin, "--threads", "3"])
            service.post("/api/capture/start", {"pid": target.pid})
            time.sleep(4.0)
            service.post("/api/capture/stop")
            time.sleep(1.5)
        status = service.get("/api/status")
        data = service.get("/api/capture/export?format=stream")
        if not isinstance(data, bytes) or not data:
            raise SystemExit("the stream export came back empty")
        return data, status
    finally:
        if target:
            target.stop()
        service.stop()


# ---------------------------------------------------------------------- site


def build(out, stream_path, bundle, name, port, service=False):
    os.makedirs(out, exist_ok=True)
    # The viewer pack, as built (build_wasm.sh). Skipped in --service mode:
    # the service already serves the viewer at /, so the site's embeds point
    # there instead of bundling a second 8+ MB copy.
    if not service:
        shutil.copytree(VIEWER_DIST, os.path.join(out, "viewer"), dirs_exist_ok=True)
    for asset in ("site.css", "logo.png", "favicon.png"):
        shutil.copy(os.path.join(HERE, asset), os.path.join(out, asset))
    # The front-page capture.
    captures = os.path.join(out, "captures")
    os.makedirs(captures, exist_ok=True)
    status = None
    if stream_path:
        data = open(stream_path, "rb").read()
    else:
        data, status = capture_stream(bundle, port)
    capture_file = f"{name}.orbit.stream"
    with open(os.path.join(captures, capture_file), "wb") as handle:
        handle.write(data)
    # Blog, screenshots, manual, test report.
    shutil.copytree(os.path.join(REPO, "docs/blog"), os.path.join(out, "blog"), dirs_exist_ok=True)
    shutil.copytree(os.path.join(REPO, "docs/screenshots"), os.path.join(out, "screenshots"), dirs_exist_ok=True)
    os.makedirs(os.path.join(out, "manual"), exist_ok=True)
    os.makedirs(os.path.join(out, "e2e"), exist_ok=True)
    pages = [
        ("docs/manual/features.md", "manual/index.html", "Orbit manual: every feature", ".."),
        ("docs/manual/live-viewer.md", "manual/live-viewer.html", "Orbit manual: the live viewer", ".."),
        ("docs/e2e/report.md", "e2e/report.html", "Orbit e2e report", ".."),
        ("docs/TODO.md", "todo.html", "Orbit TODO", "."),
    ]
    for src, dst, title, root in pages:
        path = os.path.join(REPO, src)
        if not os.path.exists(path):
            continue
        text = open(path).read()
        # Screenshot references in the docs are bare file names or ../screenshots/.
        text = text.replace("docs/screenshots/", "../screenshots/")
        body = render_markdown(text)
        body = re.sub(r'src="(\d\d-[^"]+\.png)"', r'src="../screenshots/\1"', body)
        body = re.sub(r"<code>(\d\d-[^<]+\.png)</code>", r'<a href="../screenshots/\1"><code>\1</code></a>', body)
        with open(os.path.join(out, dst), "w") as handle:
            handle.write(page(title, body, root).replace("{{capture}}", capture_file))
    # The front page.
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip()
    facts = f"{len(data) / 1024 / 1024:.1f} MB stream"
    if status:
        span = (status["newest_end_ns"] - status["oldest_start_ns"]) / 1e9
        facts = f"{status['events_live']:,} events over {span:.1f} s, {facts}"
    index = open(os.path.join(HERE, "index.html")).read()
    index = (index.replace("{{capture}}", capture_file)
             .replace("{{facts}}", html.escape(facts))
             .replace("{{commit}}", commit)
             .replace("{{date}}", time.strftime("%Y-%m-%d")))
    with open(os.path.join(out, "index.html"), "w") as handle:
        handle.write(index)
    if service:
        _point_embeds_at_service(out)
    return capture_file, facts


def _point_embeds_at_service(out):
    """Rewrite every viewer embed/link to the service's own viewer at / (and
    make the capture URLs absolute under /site), so no second viewer copy is
    shipped. Assumes the service serves this site at /site."""
    subs = [
        ("../viewer/index.html?capture=../", "/index.html?capture=/site/"),
        ("viewer/index.html?capture=../", "/index.html?capture=/site/"),
        ("../viewer/index.html", "/index.html"),
        ("viewer/index.html", "/index.html"),
    ]
    for root, _dirs, files in os.walk(out):
        for fn in files:
            if not fn.endswith(".html"):
                continue
            fp = os.path.join(root, fn)
            text = open(fp, encoding="utf-8").read()
            before = text
            for a, b in subs:
                text = text.replace(a, b)
            if text != before:
                open(fp, "w", encoding="utf-8").write(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", default=os.path.join(REPO, "site"))
    parser.add_argument("--stream", help="an .orbit.stream file to put on the front page")
    parser.add_argument("--bundle", help="an .orbit.zip to convert for the front page")
    parser.add_argument("--name", default="box3d", help="the capture's file stem on the site")
    parser.add_argument("--port", type=int, default=44850, help="port for the throwaway service")
    parser.add_argument("--service", action="store_true", help="build for embedding in orbit-service: no bundled viewer, embeds point at the service viewer at / (site served at /site)")
    args = parser.parse_args()
    if not os.path.exists(os.path.join(VIEWER_DIST, "orbit_live_viewer_bg.wasm")):
        raise SystemExit(f"no viewer pack in {VIEWER_DIST}: run src/OrbitLiveViewer/build_wasm.sh first")
    capture_file, facts = build(args.out, args.stream, args.bundle, args.name, args.port, args.service)
    print(f"site in {args.out}: front page opens captures/{capture_file} ({facts})")
    print(f"serve it:  python3 tools/site/serve.py --dir {args.out} --port 8081")


if __name__ == "__main__":
    main()
