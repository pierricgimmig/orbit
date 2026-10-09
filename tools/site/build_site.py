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

# Blog posts link up out of docs/blog/ (../rust-port-plan.html and the
# others). Copied to the site root, those relative links still resolve.
ESSAYS = (
    "rust-port-plan.html",
    "rust-service-port.html",
    "bazel-port.html",
    "uprobe-stop.html",
)


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
        if line.startswith("@clip "):
            flush_para()
            close_list()
            out.append(clip_html(line[6:].strip()))
            i += 1
            continue
        if line.startswith(">"):
            flush_para()
            close_list()
            bits = []
            while i < len(lines) and lines[i].startswith(">"):
                bits.append(lines[i][1:].strip())
                i += 1
            out.append(f'<aside class="callout"><p>{inline(" ".join(bits))}</p></aside>')
            continue
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


# Short clips already published under tools/site/media/. Width and height
# are the files' pixel size; the poster holds the box at that aspect.
CLIP_SIZE = {
    "01-attach-record": (1280, 416),
    "02-timeline-navigation": (1280, 416),
    "03-auto-profile": (1280, 800),
    "04-scheduler": (1280, 416),
    "05-flame-graph": (1280, 688),
    "06-callstacks": (1280, 768),
    "07-live-table": (1280, 800),
    "08-selection-report": (1280, 768),
    "09-service-health": (1280, 768),
    "10-code-view": (1280, 800),
    "11-scope-search": (1280, 416),
    "12-themes": (1280, 800),
    "13-mobile-compact": (488, 1000),
    "14-chrome-trace-import": (1280, 512),
}


def clip_html(spec):
    """`@clip stem | caption | aria label` → the landing page's stage."""
    parts = [p.strip() for p in spec.split("|")]
    if len(parts) < 2 or not parts[0] or not parts[1]:
        raise SystemExit(f"@clip needs 'stem | caption': {spec}")
    stem, caption = parts[0], parts[1]
    label = parts[2] if len(parts) > 2 and parts[2] else caption
    if stem not in CLIP_SIZE:
        raise SystemExit(f"unknown @clip {stem}")
    width, height = CLIP_SIZE[stem]
    phone = " phone" if stem.startswith("13-") else ""
    vid = f"clip-{stem}"
    src = f"../media/{stem}"
    return (
        f'<figure class="stage{phone}">'
        f'<div class="stage-frame">'
        f'<img class="poster" alt="" src="{src}.jpg" width="{width}" height="{height}">'
        f'<video id="{vid}" width="{width}" height="{height}" muted loop playsinline '
        f'webkit-playsinline preload="none" poster="{src}.jpg" data-src="{src}.mp4" '
        f'aria-label="{html.escape(label)}"></video>'
        f'</div>'
        f'<figcaption><span>{html.escape(caption)}</span>'
        f'<button type="button" class="clip-toggle" aria-controls="{vid}">Play</button>'
        f'</figcaption></figure>'
    )


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def inline(text):
    text = html.escape(text, quote=False)
    text = re.sub(r"!\[([^\]]*)\]\(([^)]+)\)", r'<img alt="\1" src="\2">', text)
    text = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', text)
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    text = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", text)
    return text


MANUAL_NAV = [
    ("index.html", "Quick start"),
    ("capture.html", "Capture"),
    ("timeline.html", "Timeline"),
    ("time.html", "Where time goes"),
    ("systems.html", "Systems"),
    ("everywhere.html", "Everywhere"),
    ("troubleshooting.html", "Troubleshooting"),
    ("keys.html", "Keys and mouse"),
]


def manual_toc(current):
    rows = []
    for href, label in MANUAL_NAV:
        current_attr = ' aria-current="page"' if href == current else ""
        rows.append(f'<a href="{href}"{current_attr}>{html.escape(label)}</a>')
    return "\n".join(rows)


def on_this_page(body):
    heads = re.findall(r'<h2 id="([^"]+)">(.*?)</h2>', body)
    if not heads:
        return ""
    links = []
    for anchor, title in heads:
        text = re.sub(r"<[^>]+>", "", title)
        links.append(f'<a href="#{anchor}">{text}</a>')
    return '<nav class="onpage" aria-label="On this page">' + "".join(links) + "</nav>"


def manual_page(title, body, current):
    template = open(os.path.join(HERE, "manual.html")).read()
    if "</h1>" in body:
        body = body.replace("</h1>", "</h1>\n" + on_this_page(body), 1)
    return (template.replace("{{title}}", html.escape(title))
            .replace("{{root}}", "..")
            .replace("{{body}}", body)
            .replace("{{toc}}", manual_toc(current))
            .replace("{{nav}}", NAV.replace("{{root}}", "..")))


def page(title, body, root=".", nav=True):
    template = open(os.path.join(HERE, "page.html")).read()
    return (template.replace("{{title}}", html.escape(title))
            .replace("{{root}}", root)
            .replace("{{body}}", body)
            .replace("{{nav}}", NAV.replace("{{root}}", root) if nav else ""))


NAV = ('<nav class="site" aria-label="Site"><a class="brand" href="{{root}}/index.html"><img src="{{root}}/logo.png" alt="Orbit"></a>'
       '<a href="{{root}}/index.html">Home</a>'
       '<a href="{{root}}/features.html">Features</a>'
       '<a href="{{root}}/manual/index.html">Manual</a>'
       '<a class="nav-more" href="{{root}}/blog/index.html">Blog</a>'
       '<a class="nav-more" href="{{root}}/e2e/report.html">Test report</a>'
       '<span class="spacer"></span>'
       '<button class="theme-toggle" id="theme-toggle" type="button" aria-label="Toggle colour theme">Dark</button>'
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


def site_href(page_path, out, filename):
    """A URL for a site-root file, relative to this page. Works under
    /orbit/ and at a domain root; a root-relative /favicon.ico does not,
    and without a link the browser asks the host root for one."""
    rel_dir = os.path.dirname(os.path.relpath(page_path, out))
    if rel_dir in ("", "."):
        return filename
    depth = rel_dir.count(os.sep) + 1
    return "../" * depth + filename


def inject_before_head_end(html, snippet):
    at = html.lower().find("</head>")
    if at == -1:
        return html
    return html[:at] + snippet + html[at:]


def stamp_coi(out):
    """Every document registers the isolation worker and names the site
    icon. Generated pages, the copied blog and essays, and the viewer all
    get relative URLs, so they resolve under /orbit/ and at a domain root.
    The icon link is what stops the browser requesting /favicon.ico from
    the host root (a 404 on a project site)."""
    has_icon = re.compile(r"""rel\s*=\s*["'](?:shortcut icon|icon|apple-touch-icon)["']""", re.IGNORECASE)
    for root, _dirs, files in os.walk(out):
        for fn in files:
            if not fn.endswith(".html"):
                continue
            path = os.path.join(root, fn)
            html_text = open(path, encoding="utf-8").read()
            updated = html_text
            if "coi-serviceworker.js" not in updated:
                src = site_href(path, out, "coi-serviceworker.js")
                updated = inject_before_head_end(updated, f'<script src="{src}"></script>\n')
            if not has_icon.search(updated):
                src = site_href(path, out, "favicon.png")
                updated = inject_before_head_end(
                    updated, f'<link rel="icon" type="image/png" href="{src}">\n'
                )
            if updated != html_text:
                open(path, "w", encoding="utf-8").write(updated)


_ROOT_RELATIVE = re.compile(
    r"""(?:href|src|srcset|action)\s*=\s*(['"])/(?!/)"""
    r"""|url\(\s*(['"]?)/(?!/)""",
    re.IGNORECASE,
)


def assert_no_root_relative(out):
    """A root-relative URL (/manual/...) is the host root on a custom domain
    and the user/org root on a project site. Neither is this site."""
    bad = []
    for root, _dirs, files in os.walk(out):
        for fn in files:
            if not fn.endswith((".html", ".css")):
                continue
            path = os.path.join(root, fn)
            text = open(path, encoding="utf-8", errors="replace").read()
            for i, line in enumerate(text.splitlines(), 1):
                if _ROOT_RELATIVE.search(line):
                    rel = os.path.relpath(path, out)
                    bad.append(f"{rel}:{i}: {line.strip()[:180]}")
    if bad:
        raise SystemExit(
            "root-relative URLs in the site (they break /orbit/ and a domain root):\n"
            + "\n".join(bad)
        )


def build(out, stream_path, bundle, name, port, service=False):
    os.makedirs(out, exist_ok=True)
    # The viewer pack, as built (build_wasm.sh). Skipped in --service mode:
    # the service already serves the viewer at /, so the site's embeds point
    # there instead of bundling a second 8+ MB copy.
    if not service:
        shutil.copytree(VIEWER_DIST, os.path.join(out, "viewer"), dirs_exist_ok=True)
    for asset in ("site.css", "logo.png", "favicon.png", "coi-serviceworker.js", "clips.js", "features.html"):
        shutil.copy(os.path.join(HERE, asset), os.path.join(out, asset))
    # Actions deploys the artifact as-is. .nojekyll is a no-op there and
    # keeps a later switch to the branch source from dropping files.
    open(os.path.join(out, ".nojekyll"), "w").close()
    media_src = os.path.join(HERE, "media")
    if os.path.isdir(media_src):
        shutil.copytree(media_src, os.path.join(out, "media"), dirs_exist_ok=True)
    # The curl installer, served at the site root: `curl .../install.sh | sh`.
    shutil.copy(os.path.join(REPO, "tools/install/install.sh"), os.path.join(out, "install.sh"))
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
    for name_html in ESSAYS:
        src = os.path.join(REPO, "docs", name_html)
        if os.path.exists(src):
            shutil.copy(src, os.path.join(out, name_html))
    os.makedirs(os.path.join(out, "manual"), exist_ok=True)
    os.makedirs(os.path.join(out, "e2e"), exist_ok=True)
    pages = [
        ("docs/e2e/report.md", "e2e/report.html", "Orbit e2e report", ".."),
        ("docs/TODO.md", "todo.html", "Orbit TODO", "."),
    ]
    manual_pages = [
        ("docs/manual/index.md", "index.html", "Quick start — Orbit manual"),
        ("docs/manual/capture.md", "capture.html", "Capture — Orbit manual"),
        ("docs/manual/timeline.md", "timeline.html", "Timeline — Orbit manual"),
        ("docs/manual/time.md", "time.html", "Where time goes — Orbit manual"),
        ("docs/manual/systems.md", "systems.html", "Systems — Orbit manual"),
        ("docs/manual/everywhere.md", "everywhere.html", "Everywhere — Orbit manual"),
        ("docs/manual/troubleshooting.md", "troubleshooting.html", "Troubleshooting — Orbit manual"),
        ("docs/manual/keys.md", "keys.html", "Keys and mouse — Orbit manual"),
        ("docs/manual/live-viewer.md", "live-viewer.html", "Live viewer — Orbit manual"),
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
    for src, dst, title in manual_pages:
        path = os.path.join(REPO, src)
        text = open(path, encoding="utf-8").read()
        body = render_markdown(text)
        body = re.sub(
            r'href="([A-Za-z0-9./_-]+)\.md(#[^"]*)?"',
            lambda m: f'href="{m.group(1)}.html{m.group(2) or ""}"',
            body,
        )
        with open(os.path.join(out, "manual", dst), "w", encoding="utf-8") as handle:
            handle.write(manual_page(title, body, dst).replace("{{capture}}", capture_file))
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
        # The service serves this tree at /site and its own viewer at /.
        # Those embeds are origin-root on purpose; the Pages build is not.
        _point_embeds_at_service(out)
    stamp_coi(out)
    if not service:
        assert_no_root_relative(out)
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
    print("isolation: coi-serviceworker.js adds COOP/COEP; sequential if it cannot")
    print(f"serve it:  python3 tools/site/serve.py --dir {args.out} --port 8081")


if __name__ == "__main__":
    main()
