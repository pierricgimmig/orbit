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

Crops and trims below are in source pixels and source seconds. A kit run
records most scenes at 1600x1000 with the tracks in the top of the frame, so
the same rects apply to a regenerated set. Timeline clips are cropped to the
tracks. A panel that is the subject (flame graph, call tree, report, code,
live table, a light colour scheme, the phone layout) keeps its full height.
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

# (x, y, w, h) in source pixels. w or h of 0 means the full source dimension.
# Heights sit a little below the last row of real UI, so a later kit run with
# the same 1600-wide frame gets the same crop.
CROPS = {
    "00-hero": (0, 0, 0, 560),
    "01-attach-record": (0, 0, 0, 520),
    "02-timeline-navigation": (0, 0, 0, 520),
    "04-scheduler": (0, 0, 0, 520),
    "05-flame-graph": (0, 0, 0, 860),
    "06-callstacks": (0, 0, 0, 960),
    "08-selection-report": (0, 0, 0, 960),
    "09-service-health": (0, 0, 0, 960),
    "11-scope-search": (0, 0, 0, 520),
    "14-chrome-trace-import": (0, 0, 0, 640),
}

# (start, end) in source seconds. end None runs to the end of the file.
# These drop the settings dialog ("no functions hooked") and, where the
# subject is a panel, the empty timeline that precedes it.
TRIMS = {
    "01-attach-record": (12.0, None),
    "03-auto-profile": (13.3, None),
    # The tall flame is the first moment; after that the bars collapse and the
    # lower canvas is empty.
    "05-flame-graph": (0.05, 1.45),
    "06-callstacks": (4.0, None),
    "08-selection-report": (2.0, 12.4),
    "09-service-health": (4.5, None),
    "12-themes": (3.0, 14.0),
    # Phone layout: the report and flame graph fill the frame. The earlier
    # timeline-only stretch leaves a tall empty band under the tracks.
    "13-mobile-compact": (9.2, None),
}

# Source time for the poster, when the kit's last frame is not the full UI.
POSTERS = {
    "01-attach-record": 15.2,
    "05-flame-graph": 0.6,
    "08-selection-report": 7.0,
    "09-service-health": 8.0,
    "13-mobile-compact": 11.0,
}

# Hero source ranges, in playback order. The montage opens on the timeline
# beside the flame graph, then the scheduler and nested scopes. It does not
# include the settings dialog at the start of the kit montage.
HERO_RANGES = [(14.15, 16.15), (8.15, 14.05)]
HERO_XFADE = 0.35


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


def crop_filter(stem):
    rect = CROPS.get(stem)
    if not rect:
        return None
    x, y, w, h = rect
    w = "iw" if not w else str(w)
    h = "ih" if not h else str(h)
    return f"crop={w}:{h}:{x}:{y}"


def video_filter(stem):
    parts = []
    crop = crop_filter(stem)
    if crop:
        parts.append(crop)
    parts.append(f"scale='min({MAX_W},iw)':-2")
    parts.append("format=yuv420p")
    return ",".join(parts)


def encode_video(stem, src, dest):
    cmd = ["ffmpeg", "-y", "-i", src]
    trim = TRIMS.get(stem)
    if trim:
        start, end = trim
        cmd += ["-ss", f"{start:.3f}"]
        if end is not None:
            cmd += ["-to", f"{end:.3f}"]
    cmd += [
        "-an",
        "-c:v", "libx264", "-preset", "slow", "-crf", CRF,
        "-profile:v", "high", "-pix_fmt", "yuv420p",
        "-movflags", "+faststart",
        "-vf", video_filter(stem),
        dest,
    ]
    run(cmd)


def encode_hero(src, dest_mp4, dest_jpg):
    """Open on the flame graph beside the timeline, then the scheduler."""
    (s0, e0), (s1, e1) = HERO_RANGES
    offset = (e0 - s0) - HERO_XFADE
    crop = crop_filter("00-hero")
    graph = (
        f"[0:v]{crop},setpts=PTS-STARTPTS[a];"
        f"[1:v]{crop},setpts=PTS-STARTPTS[b];"
        f"[a][b]xfade=transition=fade:duration={HERO_XFADE}:offset={offset:.3f},"
        f"scale='min({MAX_W},iw)':-2,format=yuv420p[v]"
    )
    run([
        "ffmpeg", "-y",
        "-ss", f"{s0:.3f}", "-to", f"{e0:.3f}", "-i", src,
        "-ss", f"{s1:.3f}", "-to", f"{e1:.3f}", "-i", src,
        "-filter_complex", graph,
        "-map", "[v]", "-an",
        "-c:v", "libx264", "-preset", "slow", "-crf", CRF,
        "-profile:v", "high", "-pix_fmt", "yuv420p",
        "-movflags", "+faststart",
        dest_mp4,
    ])
    # Poster is the opening frame, so the still matches the montage.
    run([
        "ffmpeg", "-y", "-i", dest_mp4,
        "-q:v", "3", "-frames:v", "1",
        dest_jpg,
    ])


def encode_poster(stem, png, mp4, dest):
    # The kit poster is the scene's last frame. A trim can move that frame
    # out of the published clip, and for a few scenes that frame is the
    # empty canvas rather than the panel. POSTERS picks a full frame.
    when = POSTERS.get(stem)
    if when is None and stem in TRIMS:
        when = TRIMS[stem][0] + 0.15
    if when is not None:
        cmd = ["ffmpeg", "-y", "-ss", f"{when:.3f}", "-i", mp4]
    else:
        cmd = ["ffmpeg", "-y", "-i", png]
    vf = video_filter(stem).replace(",format=yuv420p", "")
    run(cmd + ["-vf", vf, "-q:v", "3", "-frames:v", "1", dest])


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
        out_jpg = os.path.join(dest, stem + ".jpg")
        if stem == "00-hero":
            encode_hero(mp4, out_mp4, out_jpg)
            written.extend([stem + ".mp4", stem + ".jpg"])
            continue
        encode_video(stem, mp4, out_mp4)
        written.append(stem + ".mp4")
        if png or TRIMS.get(stem) or stem in POSTERS:
            encode_poster(stem, png or mp4, mp4, out_jpg)
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
            "Timeline clips are cropped to the tracks (`CROPS` in "
            "`publish_site_media.py`). The hero opens on the timeline beside "
            "the flame graph, and no published frame shows the empty "
            "settings dialog.\n"
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
