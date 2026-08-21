// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

const $ = (id) => document.getElementById(id);
const canvas = $("trace");
const ctx2d = canvas.getContext("2d");

let wasm = null;
let follow = true;
let viewT0 = 0;
let viewT1 = 2e9;
let dragging = false;
let dragX = 0;
let dragT0 = 0;
let latestEnd = 0;
let rendererName = "service /api/frame (2d)";

function setRenderer(name) {
  rendererName = name;
  $("renderer").textContent = name;
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
    `${s.capturing || s.demo ? (s.demo ? "DEMO" : "CAPTURING") : "idle"} · ` +
    `${s.events_live}/${s.events_capacity} events · dropped ${s.dropped} · ` +
    `spilled ${s.spilled} · produced ${s.produced} · ring ${s.ring_bytes} B`;
  $("ring").value = String(s.ring_bytes);
  if (s.spill_path) $("spill").value = s.spill_path;
  if (s.newest_end_ns) latestEnd = Number(s.newest_end_ns);
  if (follow && s.newest_end_ns > 0) {
    viewT1 = Number(s.newest_end_ns);
    viewT0 = Math.max(0, viewT1 - 2e9);
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

function resizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  canvas.width = Math.max(1, Math.floor(w * dpr));
  canvas.height = Math.max(1, Math.floor(h * dpr));
}

function paintRgba(rgba, width, lanes) {
  if (!width || !lanes) return;
  const img = ctx2d.createImageData(width, lanes);
  img.data.set(rgba);
  const tmp = document.createElement("canvas");
  tmp.width = width;
  tmp.height = lanes;
  tmp.getContext("2d").putImageData(img, 0, 0);
  ctx2d.imageSmoothingEnabled = false;
  ctx2d.fillStyle = "#0a0c10";
  ctx2d.fillRect(0, 0, canvas.width, canvas.height);
  ctx2d.drawImage(tmp, 0, 0, canvas.width, canvas.height);
}

async function drawServiceFrame() {
  const width = canvas.width || 1280;
  const qs = new URLSearchParams({
    width: String(Math.min(width, 2048)),
    t0: String(Math.floor(viewT0)),
    t1: String(Math.floor(Math.max(viewT1, viewT0 + 1))),
  });
  const res = await fetch(`/api/frame?${qs}`);
  if (!res.ok) return;
  const buf = new DataView(await res.arrayBuffer());
  if (buf.byteLength < 16) return;
  const w = buf.getUint32(0, true);
  const lanes = buf.getUint32(4, true);
  const rgba = new Uint8Array(buf.buffer, 16);
  paintRgba(rgba, w, lanes);
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
    wasm = new mod.LiveViewer();
    setRenderer("wasm pixel-columns (2d blit)");
    try {
      await wasm.init_webgpu("trace");
      setRenderer("wasm WebGPU pixel-columns");
    } catch (e) {
      console.warn("WebGPU init failed, using 2d blit", e);
    }
    return true;
  } catch (e) {
    console.warn("WASM viewer not available", e);
    wasm = null;
    setRenderer("service /api/frame (2d)");
    return false;
  }
}

function drawWasm() {
  if (!wasm) return false;
  const width = Math.min(canvas.width || 1280, 2048);
  const rgba = wasm.rasterize(viewT0, Math.max(viewT1, viewT0 + 1), width);
  const lanes = wasm.lane_count();
  if (lanes === 0 || rgba.length === 0) return true;
  if (rendererName.includes("WebGPU") && wasm.render_webgpu) {
    wasm.render_webgpu(viewT0, Math.max(viewT1, viewT0 + 1), width);
    return true;
  }
  paintRgba(rgba, width, lanes);
  return true;
}

async function frame() {
  try {
    if (!drawWasm()) {
      await drawServiceFrame();
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

window.addEventListener("resize", resizeCanvas);
resizeCanvas();

(async () => {
  await tryWasm();
  connectWs();
  await refreshStatus().catch((e) => showErr(e));
  await refreshProcesses().catch(() => {});
  setInterval(() => refreshStatus().catch(() => {}), 1000);
  requestAnimationFrame(frame);
})();
