#!/usr/bin/env node
// Record every clip of clips.mjs in ONE viewer page (headful Chrome on Xvfb, ffmpeg x11grab, lossless).
// usage: node steps/40-record.mjs --out <dir> [--only 01-attach-record,05-flame] [--skip-existing] [--serve]
//   --serve: keep the page open and accept POST /clip?id=<id> (re-imports the scene) or POST /run (JS body)
//            on 127.0.0.1:9333 for authoring scenes; POST /quit ends it.
import fs from "fs"; import path from "path"; import http from "http"; import { spawn, execSync } from "child_process";
import { fileURLToPath, pathToFileURL } from "url";
import { launch } from "../lib/harness.mjs";
const KIT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const OUT = path.resolve(arg("--out", "/workspace/orbit-media"));
const ONLY = arg("--only", "") ? arg("--only").split(",") : null;
const SKIP_EXISTING = process.argv.includes("--skip-existing");   // keep clips already recorded without error
const SERVE = process.argv.includes("--serve");
const E = process.env;
const W = +(E.REC_W || 1600), HH = +(E.REC_H || 1000), FPS = +(E.REC_FPS || 30), DPR = +(E.REC_DPR || 1.25), DISP = E.XDISPLAY || ":77";
const URL0 = `http://127.0.0.1:${E.VIEWER_PORT || 44900}/`;
const RAW = path.join(OUT, "raw"), STILLS = path.join(OUT, "stills");
fs.mkdirSync(RAW, { recursive: true }); fs.mkdirSync(STILLS, { recursive: true });
const logf = fs.createWriteStream(path.join(RAW, "record.log"), { flags: "a" });
const log = (s) => { const l = `[${new Date().toTimeString().slice(0, 8)}] ${s}`; console.log(l); logf.write(l + "\n"); };

// Xvfb (ours unless one is already on the display)
let xvfb = null;
try { execSync(`xdpyinfo -display ${DISP} >/dev/null 2>&1`); log(`using existing X display ${DISP}`); }
catch { xvfb = spawn("Xvfb", [DISP, "-screen", "0", `${W + 1}x${HH + 1}x24`, "-nolisten", "tcp"], { stdio: "ignore" }); await new Promise((r) => setTimeout(r, 1500)); log(`started Xvfb ${DISP}`); }
// pidfiles, so steps/05-stale.sh can clean up after an aborted run
const RUNDIR = path.join(KIT, ".cache", "run"); fs.mkdirSync(RUNDIR, { recursive: true });
fs.writeFileSync(path.join(RUNDIR, "recorder.pid"), String(process.pid));
if (xvfb) fs.writeFileSync(path.join(RUNDIR, "xvfb.pid"), String(xvfb.pid));
const rmPid = () => { for (const f of ["recorder", "xvfb", "chrome"]) try { fs.unlinkSync(path.join(RUNDIR, f + ".pid")); } catch {} };
for (const sig of ["SIGTERM", "SIGINT", "SIGHUP"]) process.on(sig, () => { try { xvfb?.kill(); } catch {} rmPid(); process.exit(143); });

const { clips } = await import(pathToFileURL(path.join(KIT, "clips.mjs")).href + `?t=${Date.now()}`);
const { H, close, browser } = await launch({ display: DISP, width: W, height: HH, dpr: DPR, url: URL0, chrome: E.CHROME, log });
try { const bp = browser.process?.()?.pid; if (bp) fs.writeFileSync(path.join(RUNDIR, "chrome.pid"), String(bp)); } catch {}
await H.sleep(6000);
const metaPath = path.join(RAW, "clips.json");
const meta = fs.existsSync(metaPath) ? JSON.parse(fs.readFileSync(metaPath, "utf8")) : {};
const state = {};   // shared between scenes (e.g. the capture start time)

// When clip 01 (which starts the capture from the UI) is skipped or not selected, start the same capture
// through the API so the later clips have live data, then reload the page so it shows it.
async function ensureCapture(clip) {
  if (clip.id === "01-attach-record") return;
  const st = await (await fetch(URL0 + "api/status")).json(); if (st.capturing) return;
  const ps = await (await fetch(URL0 + "api/processes")).json(); const p = ps.find((x) => x.name === "game_loop");
  if (!p) throw new Error("game_loop is not running");
  log(`no capture running before ${clip.id}: starting one through the API (pid ${p.pid}, uprobes + auto-profile, 1 kHz)`);
  await fetch(URL0 + "api/symbols/load", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ pid: p.pid }) });
  await H.sleep(2000);
  const r = await fetch(URL0 + "api/capture/start", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({
    pid: p.pid, enable_api: true, context_switches: true, thread_states: true, sampling: true, samples_per_second: 1000, unwinding: "dwarf",
    dynamic_instrumentation_method: "kernel_uprobes", auto_profile: true }) });
  log(`capture/start -> ${r.status} ${(await r.text()).slice(0, 200)}`);
  await H.goto(URL0); await H.sleep(8000);
  state.capture_started_by_api = true;
}

async function runClip(clip) {
  await ensureCapture(clip);
  const mod = await import(pathToFileURL(path.join(KIT, "scenes", clip.scene)).href + `?t=${Date.now()}`);
  if (clip.warmup_s) {   // wait until the live capture is at least this old (so reports have data)
    const s = await H.sel(); const el = s?.capture_elapsed || 0;
    if (s?.recording && el < clip.warmup_s) { log(`${clip.id}: waiting ${(clip.warmup_s - el).toFixed(0)} s for capture warm-up`); await H.sleep((clip.warmup_s - el) * 1000); }
  }
  const marks = []; const stills = [];
  const ctx = {
    clip, state, log,
    mark: (name) => marks.push({ name, t: +H.recElapsed().toFixed(2) }),        // e.g. hero in/out points
    still: async (name) => { const p = path.join(STILLS, `${name}.png`); await H.page.evaluate(() => { window.__hideCursor && window.__hideCursor(true); return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))); }); await H.sleep(600);   /* the busy canvas delays compositor frames */ H.shot(p, mod.region || null); await H.page.evaluate(() => window.__hideCursor && window.__hideCursor(false)); stills.push(p); log(`still ${p}`); },
  };
  if (mod.prep) { log(`${clip.id}: prep`); await mod.prep(H, ctx); }
  const raw = path.join(RAW, `${clip.id}.mkv`);
  log(`${clip.id}: recording`);
  await H.recStart(raw, FPS, mod.region || null);
  await H.sleep(300);
  let err = null;
  try { await mod.default(H, ctx); } catch (e) { err = String(e.stack || e); log(`${clip.id}: SCENE ERROR ${err}`); }
  const r = await H.recStop();
  if (mod.after) await mod.after(H, ctx);
  meta[clip.id] = { ...clip, raw, rec_secs: r?.secs, marks, stills, error: err, recorded_at: new Date().toISOString() };
  fs.writeFileSync(metaPath, JSON.stringify(meta, null, 2));
  log(`${clip.id}: done ${r?.secs?.toFixed(1)} s${err ? " (with error)" : ""}`);
  return meta[clip.id];
}

if (SERVE) {
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  http.createServer(async (req, res) => {
    let body = ""; req.on("data", (d) => (body += d)); await new Promise((r) => req.on("end", r));
    const u = new URL(req.url, "http://x");
    try {
      if (u.pathname === "/quit") { res.end("bye\n"); await close(); xvfb?.kill(); rmPid(); process.exit(0); }
      if (u.pathname === "/clip") {
        const { clips: cl } = await import(pathToFileURL(path.join(KIT, "clips.mjs")).href + `?t=${Date.now()}`);
        const c = cl.find((x) => x.id === u.searchParams.get("id")); if (!c) throw new Error("no clip " + u.searchParams.get("id"));
        res.end(JSON.stringify(await runClip(c), null, 1) + "\n"); return;
      }
      const out = []; const con = { log: (...a) => out.push(a.map((x) => (typeof x === "string" ? x : JSON.stringify(x))).join(" ")) };
      const r = await new AsyncFunction("H", "console", "state", body)(H, con, state);
      res.end(out.join("\n") + (r !== undefined ? "\n=> " + JSON.stringify(r) : "") + "\n");
    } catch (e) { res.statusCode = 500; res.end("ERR " + (e.stack || e) + "\n"); }
  }).listen(9333, "127.0.0.1");
  log("serve mode on 127.0.0.1:9333");
} else {
  let failed = 0;
  for (const c of clips) {
    if (ONLY && !ONLY.includes(c.id)) continue;
    const old = meta[c.id];
    if (SKIP_EXISTING && old && !old.error && old.raw && fs.existsSync(old.raw)) { log(`${c.id}: already recorded, skipping (--skip-existing)`); continue; }
    const m = await runClip(c); if (m.error) failed++;
  }
  await close(); xvfb?.kill(); rmPid();
  log(`recorded; ${failed} scene error(s)`); process.exit(failed ? 2 : 0);
}
