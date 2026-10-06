#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Publish a media-kit run into the landing page's media folder.

Reads ``NN-*.mp4`` and the matching poster ``NN-*.png`` from a directory
produced by ``./generate.sh --out <dir>``. Writes H.264 MP4 (no audio, max
width 1280, CRF 26, faststart) and JPEG posters into ``tools/site/media/``.
GIFs, WebM, raw captures and stills are not copied.

    ./generate.sh --ref <git-ref> --out /tmp/orbit-media
    python3 publish_site_media.py --from /tmp/orbit-media

Then rebuild the site (``python3 tools/site/build_site.py``). Point ``--from``
at the kit output, not at ``tools/site/media``: publishing again from the
already-compressed files would encode a second time.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
DEFAULT_TO = os.path.join(REPO, "tools", "site", "media")
MAX_W = 1280
CRF = "26"
CLIP = re.compile(r"^(\d\d-.+)\.(mp4|png)$")


def run(cmd):
    subprocess.run(cmd, check=True)


def clips(src):
    found = {}
    for name in sorted(os.listdir(src)):
        m = CLIP.match(name)
        if not m:
            continue
        found.setdefault(m.group(1), {})[m.group(2)] = os.path.join(src, name)
    return found


def source_note(src):
    path = os.path.join(src, "MANIFEST.md")
    if not os.path.isfile(path):
        return "Source manifest was not in the kit output."
    with open(path, encoding="utf-8") as handle:
        head = handle.readline().strip()
    # The manifest's first lines name the git ref. Keep the first mention.
    for line in open(path, encoding="utf-8"):
        if "da8" in line or "from `" in line or "Generated" in line:
            return line.strip()
    return head or "See the media kit manifest."


def publish(src, dest):
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg is required to publish site media")
    items = clips(src)
    if not items:
        raise SystemExit(f"no NN-*.mp4 / NN-*.png clips in {src}")
    os.makedirs(dest, exist_ok=True)
    written = []
    for stem, files in items.items():
        mp4 = files.get("mp4")
        png = files.get("png")
        if not mp4:
            print(f"skip {stem}: no mp4", file=sys.stderr)
            continue
        out_mp4 = os.path.join(dest, stem + ".mp4")
        run([
            "ffmpeg", "-y", "-i", mp4, "-an",
            "-c:v", "libx264", "-preset", "slow", "-crf", CRF,
            "-profile:v", "high", "-pix_fmt", "yuv420p",
            "-movflags", "+faststart",
            "-vf", f"scale='min({MAX_W},iw)':-2",
            out_mp4,
        ])
        written.append(stem + ".mp4")
        if png:
            out_jpg = os.path.join(dest, stem + ".jpg")
            run([
                "ffmpeg", "-y", "-i", png,
                "-vf", f"scale='min({MAX_W},iw)':-2",
                "-q:v", "3", "-frames:v", "1",
                out_jpg,
            ])
            written.append(stem + ".jpg")
        else:
            print(f"warning: {stem} has no poster png", file=sys.stderr)
    keep = set(written) | {"README.md"}
    for name in os.listdir(dest):
        if name not in keep and re.match(r"^\d\d-.+\.(mp4|jpg|png|webm|gif)$", name):
            os.remove(os.path.join(dest, name))
    note = source_note(src)
    readme = os.path.join(dest, "README.md")
    with open(readme, "w", encoding="utf-8") as handle:
        handle.write(
            "# Landing-page media\n\n"
            "Clips for `tools/site/index.html`, published by\n"
            "`tools/media-kit/publish_site_media.py`.\n"
            "Do not edit the videos by hand.\n\n"
            f"{note}\n\n"
            "Each clip is H.264, no audio, max width 1280, CRF 26, `+faststart`.\n"
            "Posters are JPEG. GIFs and WebM are not published.\n"
        )
    total = sum(os.path.getsize(os.path.join(dest, n)) for n in written)
    print(f"published {len(written)} files, {total / 1024 / 1024:.1f} MB -> {dest}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--from", dest="src", required=True, help="a generate.sh --out directory")
    parser.add_argument("--to", default=DEFAULT_TO, help="site media directory (default tools/site/media)")
    args = parser.parse_args()
    src = os.path.abspath(args.src)
    if not os.path.isdir(src):
        raise SystemExit(f"not a directory: {src}")
    publish(src, os.path.abspath(args.to))


if __name__ == "__main__":
    main()
