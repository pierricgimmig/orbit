// Harness: headful Chrome on an Xvfb display, a synthetic cursor overlay, eased mouse helpers,
// label lookup through the viewer's window.__orbit_ui, and lossless ffmpeg x11grab recording.
import pkg from "playwright-core"; import fs from "fs"; import { spawn, execFileSync } from "child_process";
const { chromium } = pkg;

const CURSOR_JS = `
(() => {
  if (window.__curInit) return; window.__curInit = 1;
  const mk = () => {
    const c = document.createElement('div');
    c.id = '__cur';
    c.style.cssText = 'position:fixed;left:0;top:0;width:26px;height:26px;z-index:2147483647;pointer-events:none;transform:translate(-100px,-100px);will-change:transform;';
    c.innerHTML = '<svg width="26" height="26" viewBox="0 0 26 26"><filter id="s" x="-50%" y="-50%" width="200%" height="200%"><feDropShadow dx="0" dy="1.2" stdDeviation="1.2" flood-opacity="0.55"/></filter><path filter="url(#s)" d="M3 2 L3 20.5 L7.7 16.3 L10.9 23.2 L14.1 21.7 L10.9 15 L17 14.6 Z" fill="#fff" stroke="#111" stroke-width="1.3" stroke-linejoin="round"/></svg>';
    document.documentElement.appendChild(c);
    const rip = document.createElement('div'); rip.id='__rip';
    rip.style.cssText='position:fixed;left:0;top:0;width:34px;height:34px;margin:-17px 0 0 -17px;border-radius:50%;border:2px solid rgba(120,190,255,.95);background:rgba(120,190,255,.18);z-index:2147483646;pointer-events:none;opacity:0;';
    document.documentElement.appendChild(rip);
    let x=-100,y=-100;
    const mv = e => { x=e.clientX; y=e.clientY; c.style.transform='translate('+(x-3)+'px,'+(y-2)+'px)'; };
    window.addEventListener('pointermove', mv, true); window.addEventListener('mousemove', mv, true);
    window.addEventListener('pointerdown', e => { if (window.__noRipple) return; rip.style.left=e.clientX+'px'; rip.style.top=e.clientY+'px';
      rip.animate([{opacity:1,transform:'scale(.35)'},{opacity:0,transform:'scale(1.25)'}],{duration:450,easing:'ease-out'}); }, true);
    window.__hideCursor = (h) => { c.style.display = h ? 'none' : ''; };
    // Hide the real X cursor: Xvfb paints it into the framebuffer (a software cursor), so x11grab's -draw_mouse 0
    // cannot drop it. Only the overlay above should be visible in recordings and stills.
    const st = document.createElement('style'); st.textContent = '*, *::before, *::after { cursor: none !important; }';
    document.documentElement.appendChild(st);
  };
  if (document.documentElement) mk(); else document.addEventListener('DOMContentLoaded', mk);
})();`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ease = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

// opts: { display, width, height, url, rawDir, chrome, log }
export async function launch(opts) {
  // PW x PH = device pixels on the X screen (what is recorded); W x HH = CSS pixels the page sees.
  // A device scale factor > 1 makes the whole UI larger, so it stays legible when downscaled for GIFs.
  const DPR = opts.dpr || 1, PW = opts.width, PH = opts.height, W = Math.round(PW / DPR), HH = Math.round(PH / DPR);
  const DISP = opts.display, LOG = opts.log || console.log;
  const browser = await chromium.launch({
    executablePath: opts.chrome || "/usr/bin/google-chrome", headless: false,
    ignoreDefaultArgs: ["--enable-automation"],
    env: { ...process.env, DISPLAY: DISP },
    args: ["--no-sandbox", "--disable-dev-shm-usage", "--use-angle=swiftshader", "--enable-webgl", "--ignore-gpu-blocklist",
      "--enable-unsafe-swiftshader", `--window-size=${PW},${PH}`, "--window-position=0,0", "--kiosk", "--disable-infobars",
      "--hide-scrollbars", `--force-device-scale-factor=${DPR}`, "--no-first-run", "--noerrdialogs", "--disable-features=Translate",
      "--disable-session-crashed-bubble", "--password-store=basic", ...(process.env.CHROME_EXTRA ? process.env.CHROME_EXTRA.split(" ") : [])],
  });
  const ctx = await browser.newContext(DPR === 1 ? { viewport: null } : { viewport: { width: W, height: HH }, deviceScaleFactor: DPR });
  await ctx.addInitScript(CURSOR_JS);
  let page = await ctx.newPage();
  const wire = (p) => {
    p.on("pageerror", (e) => LOG("PAGEERROR " + e.message + " " + String(e.stack || "").slice(0, 600).replace(/\n/g, " | ")));
    p.on("console", (m) => { const t = m.text(); if (/panicked|RuntimeError|error/i.test(t)) LOG("CONSOLE " + t.slice(0, 300)); });
  };
  wire(page);
  let pos = { x: W * 0.55, y: HH * 0.55 };
  let ff = null, ffOut = null, ffT0 = 0;
  let emu = null;   // CDP session holding a setViewport() override
  const H = {
    W, H: HH, DPR, PW, PH, sleep, log: LOG, get page() { return page; }, get pos() { return pos; },
    // --- selectors: the viewer is an egui canvas, so there is no DOM to query. The viewer publishes
    // window.__orbit_ui = [[label, x, y, w, h], ...] for every pill / tab / menu item / row it painted
    // this frame, and window.__orbit_sel (JSON state). These are the only stable hooks.
    // A label can appear more than once (e.g. tree:run_frame in several call paths): keep the first one that is on screen.
    async rects() { const t = await page.evaluate(() => window.__orbit_ui || "[]"); const m = {};
      const on = (r) => r && r[1] >= 0 && r[1] + r[3] <= HH && r[0] >= 0 && r[0] + r[2] <= W;
      for (const [l, x, y, w, h] of JSON.parse(t)) { const r = [x, y, w, h]; if (!m[l] || (!on(m[l]) && on(r))) m[l] = r; } return m; },
    // Timeline scopes carry no label: their y is derived from the thread's sample bar (layout constants of
    // this viewer build; fragile, see README "Selectors").
    async scopeY(pid, tid, depth) { const sb = await H.waitFor(`sample_bar:${pid}:${tid}`); return sb[1] + sb[3] + 9 + 21.5 * depth; },
    async labels() { return Object.keys(await H.rects()); },
    async find(label, { prefix = false, re = null } = {}) {
      const r = await H.rects(); if (!re && r[label]) return r[label];
      for (const k of Object.keys(r)) if ((re && re.test(k)) || (prefix && k.startsWith(label))) return r[k];
      return null;
    },
    async waitFor(label, { timeout = 15000, prefix = false, re = null } = {}) {
      const t0 = Date.now(); for (;;) { const r = await H.find(label, { prefix, re }); if (r) return r; if (Date.now() - t0 > timeout) throw new Error(`label not found: ${re || label}`); await sleep(200); }
    },
    async center(label, o = {}) { const r = await H.waitFor(label, o); return [r[0] + r[2] / 2, r[1] + r[3] / 2]; },
    async sel() { return JSON.parse(await page.evaluate(() => window.__orbit_sel || "null")); },
    async waitSel(pred, timeout = 20000) { const t0 = Date.now(); for (;;) { const s = await H.sel(); if (s && pred(s)) return s; if (Date.now() - t0 > timeout) throw new Error("waitSel timeout"); await sleep(250); } },
    // Is a pill "on"? Pills have no state readout, so sample the pill's pixels: a filled (on) pill is
    // painted with the accent colour, an off pill is the dark input colour. Returns mean luminance 0..255.
    async pillLum(label) {   // grabbed from the X display (a CDP screenshot makes headful Chrome flash white mid-recording)
      const r = await H.waitFor(label); const w = Math.max(2, Math.round((r[2] - 4) * DPR)), h = Math.max(2, Math.round((r[3] - 4) * DPR));
      const buf = execFileSync("ffmpeg", ["-v", "error", "-f", "x11grab", "-video_size", `${w}x${h}`, "-i", `${DISP}+${Math.round((r[0] + 2) * DPR)},${Math.round((r[1] + 2) * DPR)}`,
        "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"]);
      let t = 0; for (const v of buf) t += v; return t / buf.length;
    },
    async pillOn(label) { return (await H.pillLum(label)) > 90; },
    async ensurePill(label, on, ms = 600) { if ((await H.pillOn(label)) !== on) { await H.clickLabel(label, ms); await sleep(300); } },
    // --- eased pointer
    async glide(x, y, ms = 650) {   // time-based easing: a slow page (software GL) just gets fewer, larger steps
      const x0 = pos.x, y0 = pos.y; if (Math.hypot(x - x0, y - y0) < 1) return;
      const t0 = Date.now();
      for (;;) { const t = Math.min(1, (Date.now() - t0) / ms); const k = ease(t);
        await page.mouse.move(x0 + (x - x0) * k, y0 + (y - y0) * k); if (t >= 1) break; await sleep(12); }
      pos = { x, y };
    },
    async glideTo(label, ms = 650, o = {}) { const [x, y] = await H.center(label, o); await H.glide(x, y, ms); return [x, y]; },
    async click(x, y, ms = 650, button = "left") { await H.glide(x, y, ms); await sleep(140); await page.mouse.down({ button }); await sleep(80); await page.mouse.up({ button }); await sleep(250); },
    async clickLabel(label, ms = 650, o = {}) { const [x, y] = await H.center(label, o); await H.click(x, y, ms); return [x, y]; },
    async dblclick(x, y, ms = 650) { await H.glide(x, y, ms); await sleep(120); await page.mouse.dblclick(x, y); await sleep(250); },
    async drag(x1, y1, x2, y2, ms = 900, button = "left", mods = []) {
      await H.glide(x1, y1, 550); for (const m of mods) await page.keyboard.down(m); await sleep(120);
      await page.mouse.down({ button }); await sleep(90); await H.glide(x2, y2, ms); await sleep(120); await page.mouse.up({ button });
      for (const m of mods) await page.keyboard.up(m); await sleep(200);
    },
    async wheel(n, dy = -100, every = 70, mods = []) { for (const m of mods) await page.keyboard.down(m); for (let i = 0; i < n; i++) { await page.mouse.wheel(0, dy); await sleep(every); } for (const m of mods) await page.keyboard.up(m); },
    async hold(key, ms) { await page.keyboard.down(key); await sleep(ms); await page.keyboard.up(key); },
    async type(s, delay = 80) { await page.keyboard.type(s, { delay }); },
    async key(k) { await page.keyboard.press(k); },
    async shot(path, region = null) {   // full-res PNG straight from the X display (no CDP screenshot: it flashes headful Chrome)
      const vw = region ? Math.round(region.w * DPR / 2) * 2 : PW, vh = region ? Math.round(region.h * DPR / 2) * 2 : PH;
      execFileSync("ffmpeg", ["-v", "error", "-y", "-f", "x11grab", "-draw_mouse", "0", "-video_size", `${vw}x${vh}`, "-i", `${DISP}+0,0`, "-frames:v", "1", path]); return path; },
    // --- recording (lossless RGB H.264 in .mkv; converted later)
    async recStart(out, fps = 30, region = null) {   // region: {w, h} in CSS px from the top-left, default the whole page
      if (ff) await H.recStop();
      const vw = region ? Math.round(region.w * DPR / 2) * 2 : PW, vh = region ? Math.round(region.h * DPR / 2) * 2 : PH;
      ffOut = out; ffT0 = Date.now();
      ff = spawn("ffmpeg", ["-y", "-loglevel", "error", "-f", "x11grab", "-draw_mouse", "0", "-framerate", String(fps), "-video_size", `${vw}x${vh}`,
        "-i", `${DISP}+0,0`, "-c:v", "libx264rgb", "-preset", "ultrafast", "-crf", "0", "-g", "60", out], { stdio: ["pipe", "inherit", "inherit"] });
      await sleep(400); return out;
    },
    async recStop() { if (!ff) return null; const p = ff; ff = null; p.stdin.write("q"); await new Promise((r) => p.on("exit", r)); return { out: ffOut, secs: (Date.now() - ffT0) / 1000 }; },
    recElapsed() { return ff ? (Date.now() - ffT0) / 1000 : 0; },
    // --- navigation (only ever ONE viewer page: each new viewer costs the service 0.4-0.85 GB in a 2 GB VM)
    async goto(url) { await page.goto(url, { waitUntil: "load", timeout: 60000 }); await page.mouse.move(pos.x, pos.y); },
    async reopen(url) { await page.close(); page = await ctx.newPage(); wire(page); await H.goto(url); },
    // Emulated viewport through CDP (Playwright's setViewportSize resizes the headful window and drops fullscreen).
    // The override only lives as long as its CDP session, so the session is kept open until the viewport is restored to W x HH.
    async setViewport(w, h) {
      if (w === W && h === HH) { if (emu) { await emu.send("Emulation.clearDeviceMetricsOverride").catch(() => {}); await emu.detach().catch(() => {}); emu = null; } return; }
      emu ??= await page.context().newCDPSession(page);
      await emu.send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: DPR, mobile: false }); },
    async fullscreen() { const c = await ctx.newCDPSession(page); const { windowId } = await c.send("Browser.getWindowForTarget");
      await c.send("Browser.setWindowBounds", { windowId, bounds: { windowState: "fullscreen" } }); await c.detach(); await sleep(800); },
  };
  // Fullscreen through CDP (no window manager on Xvfb, so --kiosk alone leaves the tab strip).
  // Chrome's fullscreen on a bare X screen is (screen - 1px), so the Xvfb screen is created 1px larger.
  { const c = await ctx.newCDPSession(page); const { windowId } = await c.send("Browser.getWindowForTarget");
    await c.send("Browser.setWindowBounds", { windowId, bounds: { windowState: "fullscreen" } }); await c.detach(); await sleep(1200); }
  await H.goto(opts.url);
  const vp = await page.evaluate(() => [innerWidth, innerHeight]);
  if (vp[0] !== W || vp[1] !== HH) LOG(`WARNING viewport is ${vp[0]}x${vp[1]}, expected ${W}x${HH}`);
  return { browser, ctx, H, close: async () => { await H.recStop(); await browser.close(); } };
}
