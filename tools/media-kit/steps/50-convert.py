#!/usr/bin/env python3
"""Convert raw lossless recordings into web media.
usage: steps/50-convert.py --out <dir> [--only id,id] [--no-hero]
Per clip (from <out>/raw/clips.json, written by 40-record.mjs):
  NN-name.mp4  H.264 High, yuv420p, CRF 18, +faststart, recorded resolution, 30 fps
  NN-name.webm VP9 CRF 32 (constrained quality), row-mt
  NN-name.gif  gifski, GIF_W px wide, GIF_FPS fps; quality / fps / width step down until <= GIF_MAX_MB
  NN-name.png  poster frame (full resolution) at the clip's hero_in mark (or 40% in)
and 00-hero.{mp4,webm,gif,png}: the hero_in..hero_out windows of clips with hero: true, crossfaded.
Env: GIF_W (960) GIF_FPS (12) GIF_MAX_MB (4) HERO_SEG_S (3.6) HERO_GIF_W (800) HERO_GIF_MAX_MB (8)."""
import json, os, shutil, subprocess, sys, tempfile, argparse, glob

E = os.environ
GIF_W = int(E.get("GIF_W", 960)); GIF_FPS = int(E.get("GIF_FPS", 12)); GIF_MAX = float(E.get("GIF_MAX_MB", 4))
HERO_SEG = float(E.get("HERO_SEG_S", 3.6)); HERO_GIF_W = int(E.get("HERO_GIF_W", 800)); HERO_GIF_MAX = float(E.get("HERO_GIF_MAX_MB", 8))
LEAD = 0.25   # skip the first frames (recorder start-up)

def run(cmd, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, **kw)
    if r.returncode: sys.stderr.write(" ".join(cmd) + "\n" + r.stderr[-2000:]); raise SystemExit(f"command failed: {cmd[0]}")
    return r.stdout

def probe_dur(p): return float(run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", p]).strip())

TARGET_S = float(E.get("TARGET_S", 15))   # clips longer than this play faster (up to MAX_SPEED) to land near it
MAX_SPEED = float(E.get("MAX_SPEED", 1.75))

def vf_for(clip, speed=1.0):
    f = [f"setpts=PTS/{speed:.4f}"] if abs(speed - 1) > 1e-3 else []
    if clip.get("crop"):   # [x, y, w, h] in recorded (device) pixels
        x, y, w, h = clip["crop"]; f.append(f"crop={w}:{h}:{x}:{y}")
    return f

def encode_mp4_webm(src, ss, t, vf, stem):
    vfs = ",".join(vf + ["format=yuv420p"])
    run(["ffmpeg", "-v", "error", "-y", "-ss", f"{ss:.3f}", "-t", f"{t:.3f}", "-i", src, "-vf", vfs, "-r", "30",
         "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-profile:v", "high", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-an", stem + ".mp4"])
    run(["ffmpeg", "-v", "error", "-y", "-ss", f"{ss:.3f}", "-t", f"{t:.3f}", "-i", src, "-vf", vfs, "-r", "30",
         "-c:v", "libvpx-vp9", "-crf", "32", "-b:v", "0", "-row-mt", "1", "-deadline", "good", "-cpu-used", "2", "-pix_fmt", "yuv420p", "-an", stem + ".webm"])

def make_gif(src, ss, t, vf, out, width, max_mb):
    """gifski from PNG frames; step quality, then fps, then width down until it fits."""
    tries = [(90, GIF_FPS, width), (80, GIF_FPS, width), (70, GIF_FPS, width), (70, 10, width), (65, 10, int(width * 0.85)), (60, 8, int(width * 0.75))]
    best = None
    for q, fps, w in tries:
        with tempfile.TemporaryDirectory() as d:
            run(["ffmpeg", "-v", "error", "-y", "-ss", f"{ss:.3f}", "-t", f"{t:.3f}", "-i", src,
                 "-vf", ",".join(vf + [f"fps={fps}", f"scale={w}:-2:flags=lanczos"]), os.path.join(d, "f%05d.png")])
            frames = sorted(glob.glob(os.path.join(d, "f*.png")))
            run(["gifski", "--quiet", "--fps", str(fps), "--quality", str(q), "--width", str(w), "-o", out] + frames)
        mb = os.path.getsize(out) / 1e6; best = (q, fps, w, mb)
        if mb <= max_mb: break
    return best

def probe_width(src):
    return int(run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width", "-of", "csv=p=0", src]).strip())

def poster(src, at, vf, out):
    run(["ffmpeg", "-v", "error", "-y", "-ss", f"{at:.3f}", "-i", src, "-frames:v", "1"] + (["-vf", ",".join(vf)] if vf else []) + [out])

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--out", required=True); ap.add_argument("--only", default=""); ap.add_argument("--no-hero", action="store_true")
    a = ap.parse_args(); out = os.path.abspath(a.out); only = [x for x in a.only.split(",") if x]
    meta = json.load(open(os.path.join(out, "raw", "clips.json")))
    sys.path.insert(0, os.path.dirname(__file__))
    clips = json.loads(run(["node", "-e", f"import('{os.path.dirname(os.path.abspath(__file__))}/../clips.mjs').then(m=>console.log(JSON.stringify(m.clips)))"]))
    order = [c["id"] for c in clips]
    report = {}
    for cid in order:
        if cid not in meta or (only and cid not in only): continue
        m = {**meta[cid], **next(c for c in clips if c["id"] == cid)}   # clips.mjs wins (captions/crops can change without re-recording)
        src = m["raw"]; dur = probe_dur(src)
        src_t = min(dur - LEAD, float(m.get("max_s") or dur))           # seconds of the recording used
        speed = float(m["speed"]) if m.get("speed") else min(MAX_SPEED, max(1.0, src_t / TARGET_S))
        t = src_t; vf = vf_for(m, speed); stem = os.path.join(out, cid)
        marks = {k["name"]: k["t"] for k in m.get("marks", [])}
        print(f"[convert] {cid}: {t:.1f} s of {dur:.1f} s at {speed:.2f}x", flush=True)
        encode_mp4_webm(src, LEAD, t, vf, stem)
        src_w = int(m["crop"][2]) if m.get("crop") else probe_width(src)  # never upscale (e.g. the 390 px phone clip)
        g = make_gif(src, LEAD, t, vf, stem + ".gif", min(GIF_W, src_w // 2 * 2), GIF_MAX)
        pa = marks.get("still_at", marks.get("hero_out", t * 0.6)) - 0.2
        poster(src, max(LEAD, min(pa, LEAD + t - 0.1)), vf_for(m), stem + ".png")
        report[cid] = {"speed": round(speed, 2), "gif": {"quality": g[0], "fps": g[1], "width": g[2], "mb": round(g[3], 2)}}
        print(f"[convert]   gif q{g[0]} {g[1]}fps {g[2]}px {g[3]:.2f} MB", flush=True)
    if not a.no_hero and not only:
        segs = []
        for c in clips:
            if not c.get("hero") or c["id"] not in meta: continue
            mk = {k["name"]: k["t"] for k in meta[c["id"]].get("marks", [])}
            if "hero_in" not in mk: continue
            s0 = max(LEAD, mk["hero_in"]); s1 = min(mk.get("hero_out", s0 + HERO_SEG), s0 + HERO_SEG)
            segs.append((meta[c["id"]]["raw"], s0, s1 - s0))
        if segs:
            print(f"[convert] hero: {len(segs)} segments", flush=True)
            X = 0.35; inputs = []; fl = []
            for i, (src, s0, d) in enumerate(segs):
                inputs += ["-ss", f"{s0:.3f}", "-t", f"{d:.3f}", "-i", src]
                fl.append(f"[{i}:v]fps=30,scale=1600:1000:force_original_aspect_ratio=decrease,pad=1600:1000:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v{i}]")
            acc = "v0"; off = segs[0][2]
            for i in range(1, len(segs)):
                off -= X; fl.append(f"[{acc}][v{i}]xfade=transition=fade:duration={X}:offset={off:.3f}[x{i}]"); acc = f"x{i}"; off += segs[i][2]
            hero_raw = os.path.join(out, "raw", "00-hero.mkv")
            run(["ffmpeg", "-v", "error", "-y"] + inputs + ["-filter_complex", ";".join(fl), "-map", f"[{acc}]", "-c:v", "libx264", "-crf", "8", "-preset", "fast", hero_raw])
            hd = probe_dur(hero_raw); stem = os.path.join(out, "00-hero")
            encode_mp4_webm(hero_raw, 0, hd, [], stem)
            g = make_gif(hero_raw, 0, hd, [], stem + ".gif", HERO_GIF_W, HERO_GIF_MAX)
            poster(hero_raw, segs[0][2] * 0.8, [], stem + ".png")
            report["00-hero"] = {"segments": [(os.path.basename(s), round(a, 2), round(d, 2)) for s, a, d in segs], "gif": {"quality": g[0], "fps": g[1], "width": g[2], "mb": round(g[3], 2)}}
            print(f"[convert] hero {hd:.1f} s, gif {g[3]:.2f} MB", flush=True)
    rp = os.path.join(out, "raw", "convert.json"); old = json.load(open(rp)) if os.path.exists(rp) else {}
    old.update(report); json.dump(old, open(rp, "w"), indent=1)

main()
