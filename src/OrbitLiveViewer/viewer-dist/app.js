// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

const $ = (id) => document.getElementById(id);
const canvas = $("trace");
const ctx2d = canvas.getContext("2d");
const ticks = $("ticks");
const tctx = ticks.getContext("2d");

const CHROME = {
  canvas: "#434343",
  track: "#323232",
  timebar: "#212021",
  tick: "#FFFEFD",
  playhead: "rgba(255,255,255,0.5)",
};

const usp = new URLSearchParams(location.search);
const forcedSpan = Number(usp.get("span") || 0);

let wasm = null;
let follow = forcedSpan <= 0;
let viewT0 = 0;
let viewT1 = 2e9;
let dragging = false;
let dragX = 0;
let dragT0 = 0;
let latestEnd = 0;
let rendererName = "service /api/timeline (2d)";
let lastLod = "";

function setRenderer(name) {
  rendererName = name;
  $("renderer").textContent = name + (lastLod ? ` · ${lastLod}` : "");
}

function showErr(msg) {
  $("err").textContent = msg || "";
}

async function api(path, opts) {
  const res = await fetch(path, opts);
  if (!res.ok) {
    throw new Error(`${path}: ${res.status} ${await res.text()}`);
  }
  const ct = res.headers.get("content-type") || "";
  if (ct.includes("json")) return res.json();
  return res;
}

async function refreshStatus() {
  const s = await api("/api/status");
  $("status").textContent =
    `${s.capturing || s.demo ? (s.demo ? "DEMO" : "CAPTURING") : "idle"}\n` +
    `${s.events_live}/${s.events_capacity} live\n` +
    `dropped ${s.dropped}  spilled ${s.spilled}\n` +
    `produced ${s.produced}\n` +
    `ring ${s.ring_bytes} B`;
  $("ring").value = String(s.ring_bytes);
  if (s.spill_path) $("spill").value = s.spill_path;
  if (s.newest_end_ns) latestEnd = Number(s.newest_end_ns);
  if (s.newest_end_ns > 0 && (follow || forcedSpan > 0)) {
    viewT1 = Number(s.newest_end_ns);
    viewT0 = Math.max(0, viewT1 - (forcedSpan > 0 ? forcedSpan : 2e9));
  }
  return s;
}

async function refreshProcesses() {
  const list = await api("/api/processes");
  const sel = $("processes");
  const prev = sel.value;
  sel.innerHTML = "";
  for (const p of list) {
    const opt = document.createElement("option");
    opt.value = p.pid;
    opt.textContent = `${p.pid}  ${p.name || ""}`;
    sel.appendChild(opt);
  }
  if (prev) sel.value = prev;
}

async function startCapture() {
  const pid = Number($("processes").value);
  if (!pid) throw new Error("Select a process first (or use Start demo).");
  await api("/api/capture/start", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      pid,
      enable_api: true,
      context_switches: true,
      thread_states: true,
    }),
  });
  follow = true;
}

async function stopCapture() {
  await api("/api/capture/stop", { method: "POST" });
}

async function startDemo() {
  await api("/api/demo/start", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ scopes_per_sec: 40000 }),
  });
  follow = true;
}

async function stopDemo() {
  await api("/api/demo/stop", { method: "POST" });
}

async function applyConfig() {
  await api("/api/config", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      ring_buffer_bytes: Number($("ring").value),
      spill_path: $("spill").value || null,
    }),
  });
}

function cssRgb(hex) {
  const h = hex.replace("#", "");
  const n = parseInt(h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function paintTimebar(cssW, cssH, dpr) {
  ticks.width = Math.max(1, Math.floor(cssW * dpr));
  ticks.height = Math.max(1, Math.floor(cssH * dpr));
  tctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  tctx.fillStyle = CHROME.timebar;
  tctx.fillRect(0, 0, cssW, cssH);
  const span = Math.max(1, viewT1 - viewT0);
  const nice = niceStep(span);
  tctx.strokeStyle = "rgba(255,254,253,0.25)";
  tctx.fillStyle = CHROME.tick;
  tctx.font = "10px sans-serif";
  const tStart = Math.floor(viewT0 / nice) * nice;
  for (let t = tStart; t < viewT1; t += nice) {
    const x = ((t - viewT0) / span) * cssW;
    tctx.beginPath();
    tctx.moveTo(x + 0.5, 0);
    tctx.lineTo(x + 0.5, cssH);
    tctx.stroke();
    tctx.fillText(fmtTime(t), x + 3, 14);
  }
  if (latestEnd >= viewT0 && latestEnd <= viewT1) {
    const x = ((latestEnd - viewT0) / span) * cssW;
    tctx.strokeStyle = CHROME.playhead;
    tctx.beginPath();
    tctx.moveTo(x + 0.5, 0);
    tctx.lineTo(x + 0.5, cssH);
    tctx.stroke();
  }
}

function niceStep(spanNs) {
  const steps = [1e3, 2e3, 5e3, 1e4, 2e4, 5e4, 1e5, 2e5, 5e5, 1e6, 2e6, 5e6, 1e7, 2e7, 5e7, 1e8, 2e8, 5e8, 1e9, 2e9];
  const target = spanNs / 8;
  for (const s of steps) if (s >= target) return s;
  return 5e9;
}

function fmtTime(ns) {
  if (ns >= 1e9) return (ns / 1e9).toFixed(3) + "s";
  if (ns >= 1e6) return (ns / 1e6).toFixed(2) + "ms";
  if (ns >= 1e3) return (ns / 1e3).toFixed(1) + "us";
  return ns + "ns";
}

function roundShadow(x, y, w, h, r, color) {
  const [cr, cg, cb] = cssRgb(color);
  ctx2d.save();
  ctx2d.shadowColor = "rgba(0,0,0,0.45)";
  ctx2d.shadowBlur = 3;
  ctx2d.shadowOffsetX = 0.8;
  ctx2d.shadowOffsetY = 1.2;
  ctx2d.fillStyle = color;
  roundRect(x, y, w, h, r);
  ctx2d.fill();
  ctx2d.shadowColor = "transparent";
  ctx2d.fillStyle = `rgb(${Math.round(cr * 0.94)},${Math.round(cg * 0.94)},${Math.round(cb * 0.94)})`;
  ctx2d.fillRect(x, y, Math.min(3, w), h);
  ctx2d.strokeStyle = "rgba(255,255,255,0.35)";
  ctx2d.lineWidth = 1;
  roundRect(x + 0.5, y + 0.5, Math.max(0, w - 1), Math.max(0, h - 1), r);
  ctx2d.stroke();
  ctx2d.restore();
}

function roundRect(x, y, w, h, r) {
  const rr = Math.min(r, w / 2, h / 2);
  ctx2d.beginPath();
  ctx2d.moveTo(x + rr, y);
  ctx2d.lineTo(x + w - rr, y);
  ctx2d.quadraticCurveTo(x + w, y, x + w, y + rr);
  ctx2d.lineTo(x + w, y + h - rr);
  ctx2d.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
  ctx2d.lineTo(x + rr, y + h);
  ctx2d.quadraticCurveTo(x, y + h, x, y + h - rr);
  ctx2d.lineTo(x, y + rr);
  ctx2d.quadraticCurveTo(x, y, x + rr, y);
  ctx2d.closePath();
}

function paintRgbaLanes(rgba, width, lanes, laneH) {
  const wrap = $("trace-wrap");
  const cssW = wrap.clientWidth;
  const h = Math.max(lanes * laneH, 64);
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.max(1, Math.floor(cssW * dpr));
  canvas.height = Math.max(1, Math.floor(h * dpr));
  canvas.style.width = cssW + "px";
  canvas.style.height = h + "px";
  ctx2d.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx2d.fillStyle = CHROME.canvas;
  ctx2d.fillRect(0, 0, cssW, h);
  if (!width || !lanes) return;
  const img = ctx2d.createImageData(width, lanes);
  img.data.set(rgba);
  const tmp = document.createElement("canvas");
  tmp.width = width;
  tmp.height = lanes;
  tmp.getContext("2d").putImageData(img, 0, 0);
  ctx2d.imageSmoothingEnabled = false;
  for (let i = 0; i < lanes; i++) {
    ctx2d.drawImage(tmp, 0, i, width, 1, 0, i * laneH, cssW, laneH - 1);
  }
}

function drawPlayhead(cssW, h) {
  if (latestEnd < viewT0 || latestEnd > viewT1) return;
  const x = ((latestEnd - viewT0) / Math.max(1, viewT1 - viewT0)) * cssW;
  ctx2d.strokeStyle = CHROME.playhead;
  ctx2d.beginPath();
  ctx2d.moveTo(x + 0.5, 0);
  ctx2d.lineTo(x + 0.5, h);
  ctx2d.stroke();
}

async function drawServiceTimeline() {
  const wrap = $("trace-wrap");
  const cssW = wrap.clientWidth;
  const dpr = window.devicePixelRatio || 1;
  paintTimebar(cssW, 22, dpr);
  const width = Math.min(Math.max(16, Math.floor(cssW * dpr)), 2048);
  const qs = new URLSearchParams({
    width: String(width),
    t0: String(Math.floor(viewT0)),
    t1: String(Math.floor(Math.max(viewT1, viewT0 + 1))),
  });
  const tl = await api(`/api/timeline?${qs}`);
  lastLod = tl.lod;
  if (tl.lod === "instanced" && tl.instances && tl.instances.length) {
    const scale = cssW / Math.max(1, tl.width);
    canvas.width = Math.max(1, Math.floor(cssW * dpr));
    canvas.height = Math.max(1, Math.floor(Math.max(tl.height, 64) * dpr));
    canvas.style.width = cssW + "px";
    canvas.style.height = Math.max(tl.height, 64) + "px";
    ctx2d.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx2d.fillStyle = CHROME.canvas;
    ctx2d.fillRect(0, 0, cssW, Math.max(tl.height, 64));
    for (const inst of tl.instances) {
      roundShadow(inst.x * scale, inst.y, inst.w * scale, inst.h, inst.r, inst.color);
    }
    drawPlayhead(cssW, Math.max(tl.height, 64));
    setRenderer("service /api/timeline (2d rounded)");
    return;
  }
  const res = await fetch(`/api/frame?${qs}`);
  if (!res.ok) return;
  const buf = new DataView(await res.arrayBuffer());
  if (buf.byteLength < 16) return;
  const w = buf.getUint32(0, true);
  const lanes = buf.getUint32(4, true);
  const rgbaBytes = w * lanes * 4;
  if (rgbaBytes === 0 || buf.byteLength < 16 + rgbaBytes) return;
  const rgba = new Uint8Array(buf.buffer, buf.byteOffset + 16, rgbaBytes);
  paintRgbaLanes(rgba, w, lanes, 14);
  drawPlayhead(cssW, Math.max(lanes * 14, 64));
  setRenderer("service /api/frame (2d columns)");
}

function connectWs() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.binaryType = "arraybuffer";
  ws.onmessage = (ev) => {
    if (!wasm || !(ev.data instanceof ArrayBuffer)) return;
    wasm.ingest(new Uint8Array(ev.data));
    const bounds = wasm.time_bounds();
    if (bounds && bounds.length === 2) {
      latestEnd = bounds[1];
      if (follow) {
        viewT1 = bounds[1];
        viewT0 = Math.max(bounds[0], viewT1 - 2e9);
      }
    }
  };
  ws.onclose = () => setTimeout(connectWs, 1000);
}

async function tryWasm() {
  try {
    const mod = await import("./orbit_live_viewer.js");
    await mod.default();
    if (typeof SharedArrayBuffer !== "undefined" && typeof mod.initThreadPool === "function") {
      try {
        // See index.html: the useful pool width is bounded by the per-frame
        // collect/raster work, not the core count. ?threads=N overrides.
        const hw = Number(navigator.hardwareConcurrency) || 4;
        const forced = Number(new URLSearchParams(location.search).get("threads"));
        const want = Number.isFinite(forced) && forced > 0 ? forced : Math.min(hw, 8);
        const n = Math.max(1, Math.min(want, 32));
        await mod.initThreadPool(n);
        if (typeof mod.markWasmPoolReady === "function") {
          mod.markWasmPoolReady(n);
        }
      } catch (poolErr) {
        console.warn("WASM lane pool unavailable; sequential collect/raster", poolErr);
      }
    }
    wasm = new mod.LiveViewer();
    setRenderer("wasm pixel-columns (2d blit)");
    try {
      if (mod.init_webgpu) {
        await mod.init_webgpu("trace");
        setRenderer("wasm WebGPU hybrid");
      }
    } catch (e) {
      console.warn("WebGPU init failed, using 2d", e);
    }
    return true;
  } catch (e) {
    console.warn("WASM viewer not available", e);
    wasm = null;
    setRenderer("service /api/timeline (2d)");
    return false;
  }
}

function unpackInstances(packed) {
  const view = new DataView(packed.buffer, packed.byteOffset, packed.byteLength);
  if (packed.byteLength < 8) return { height: 64, list: [] };
  const height = view.getFloat32(0, true);
  const count = view.getUint32(4, true);
  const list = [];
  let o = 8;
  for (let i = 0; i < count && o + 24 <= packed.byteLength; i++) {
    const x = view.getFloat32(o, true);
    const y = view.getFloat32(o + 4, true);
    const w = view.getFloat32(o + 8, true);
    const h = view.getFloat32(o + 12, true);
    const color = view.getUint32(o + 16, true);
    const r = view.getFloat32(o + 20, true);
    const hex = "#" + ((color >>> 16) & 255).toString(16).padStart(2, "0")
      + ((color >>> 8) & 255).toString(16).padStart(2, "0")
      + (color & 255).toString(16).padStart(2, "0");
    list.push({ x, y, w, h, r, color: hex });
    o += 24;
  }
  return { height, list };
}

function drawWasm() {
  if (!wasm) return false;
  const wrap = $("trace-wrap");
  const cssW = wrap.clientWidth;
  const dpr = window.devicePixelRatio || 1;
  paintTimebar(cssW, 22, dpr);
  const width = Math.min(Math.max(16, Math.floor(cssW * dpr)), 2048);
  const t0 = viewT0;
  const t1 = Math.max(viewT1, viewT0 + 1);
  let lod = "pixel_columns";
  if (typeof wasm.choose_lod === "function") {
    lod = wasm.choose_lod(t0, t1, width) === 1 ? "instanced" : "pixel_columns";
  }
  lastLod = lod;
  if (lod === "instanced" && typeof wasm.collect_instances === "function") {
    if (rendererName.includes("WebGPU") && wasm.render_webgpu) {
      try {
        wasm.render_webgpu(t0, t1, width);
        setRenderer("wasm WebGPU instanced SDF");
        return true;
      } catch (e) {
        console.warn(e);
      }
    }
    const packed = wasm.collect_instances(t0, t1, width);
    const inst = unpackInstances(packed);
    const height = inst.height || 64;
    canvas.width = Math.max(1, Math.floor(cssW * dpr));
    canvas.height = Math.max(1, Math.floor(height * dpr));
    canvas.style.width = cssW + "px";
    canvas.style.height = height + "px";
    ctx2d.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx2d.fillStyle = CHROME.canvas;
    ctx2d.fillRect(0, 0, cssW, height);
    const scale = cssW / Math.max(1, width);
    for (const i of inst.list) {
      roundShadow(i.x * scale, i.y, i.w * scale, i.h, i.r, i.color);
    }
    setRenderer("wasm instanced (2d)");
    return true;
  }
  if (rendererName.includes("WebGPU") && wasm.render_webgpu) {
    wasm.render_webgpu(t0, t1, width);
    setRenderer("wasm WebGPU pixel-columns");
    return true;
  }
  const rgba = wasm.rasterize(t0, t1, width);
  const lanes = wasm.lane_count();
  if (lanes === 0 || rgba.length === 0) return true;
  paintRgbaLanes(rgba, width, lanes, 14);
  setRenderer("wasm pixel-columns (2d blit)");
  return true;
}

async function frame() {
  try {
    if (!drawWasm()) {
      await drawServiceTimeline();
    }
  } catch (e) {
    showErr(String(e));
  }
  requestAnimationFrame(frame);
}

canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  follow = false;
  const span = viewT1 - viewT0;
  const factor = e.deltaY > 0 ? 1.15 : 1 / 1.15;
  const rect = canvas.getBoundingClientRect();
  const x = (e.clientX - rect.left) / rect.width;
  const center = viewT0 + span * x;
  const next = Math.max(1e3, span * factor);
  viewT0 = Math.max(0, center - next * x);
  viewT1 = viewT0 + next;
}, { passive: false });

canvas.addEventListener("mousedown", (e) => {
  dragging = true;
  dragX = e.clientX;
  dragT0 = viewT0;
  follow = false;
});
window.addEventListener("mouseup", () => { dragging = false; });
window.addEventListener("mousemove", (e) => {
  if (!dragging) return;
  const span = viewT1 - viewT0;
  const dx = (e.clientX - dragX) / canvas.clientWidth;
  viewT0 = Math.max(0, dragT0 - dx * span);
  viewT1 = viewT0 + span;
});
window.addEventListener("keydown", (e) => {
  if (e.code === "Space") {
    e.preventDefault();
    follow = true;
  }
});

$("refresh").onclick = () => refreshProcesses().catch((e) => showErr(e));
$("start-capture").onclick = () => startCapture().catch((e) => showErr(e));
$("stop-capture").onclick = () => stopCapture().catch((e) => showErr(e));
$("start-demo").onclick = () => startDemo().catch((e) => showErr(e));
$("stop-demo").onclick = () => stopDemo().catch((e) => showErr(e));
$("apply-cfg").onclick = () => applyConfig().catch((e) => showErr(e));

(async () => {
  await tryWasm();
  connectWs();
  await refreshStatus().catch((e) => showErr(e));
  await refreshProcesses().catch(() => {});
  setInterval(() => refreshStatus().catch(() => {}), 1000);
  requestAnimationFrame(frame);
})();
