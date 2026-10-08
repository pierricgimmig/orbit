// Helpers shared by scenes. Everything is found through window.__orbit_ui labels where the viewer
// provides one; positions derived from layout constants are marked FRAGILE (see README).
export const VIEWER = () => `http://127.0.0.1:${process.env.VIEWER_PORT || 44900}`;
export async function api(path, body) {
  const r = await fetch(VIEWER() + path, body ? { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) } : {});
  return r.json();
}
export async function targetPid() {
  const ps = await api("/api/processes"); const p = ps.find((x) => x.name === "game_loop");
  if (!p) throw new Error("game_loop is not running in the guest"); return p.pid;
}
export async function threadIds(H) {   // [pid, tid] of the game_loop main thread row
  const pid = await targetPid(); return [pid, pid];
}
// Toggled through More > Report (the R shortcut is swallowed when a text box has keyboard focus).
export async function reportOpen(H, open, ms = 1) {
  for (let i = 0; i < 2; i++) {
    const s = await H.sel(); if (!!s.report_open === open) return;
    await H.clickLabel("More", ms); await H.sleep(500); await H.clickLabel("Report", ms); await H.sleep(800);
  }
}
export async function settingsOpen(H) { return !!(await H.find("CSW")); }
export async function setSettings(H, open, ms = 600) { if ((await settingsOpen(H)) !== open) { await H.clickLabel("Settings", ms); await H.sleep(700); } }
export async function follow(H, on) {   // Follow state is only readable from the More menu label "Follow:on|off"
  await H.page.keyboard.press("Escape").catch(() => {});
  await H.clickLabel("More", 1); await H.sleep(500);
  const r = await H.find("Follow:", { prefix: true }); const cur = r ? (await H.labels()).find((l) => l.startsWith("Follow:")) : null;
  await H.key("Escape"); await H.sleep(300);
  if (cur && (cur === "Follow:on") !== on) { await H.key(" "); await H.sleep(300); }
  return cur;
}
export async function canvasCenterX(H) { const sb = await H.find("sample_bar:", { prefix: true }); return sb ? sb[0] + sb[2] / 2 : H.W / 2; }
export async function timelineRight(H) { const sb = await H.find("sample_bar:", { prefix: true }); return sb ? sb[0] + sb[2] : H.W - 20; }
export async function timelineLeft(H) { const sb = await H.find("sample_bar:", { prefix: true }); return sb ? sb[0] : 200; }
// Zoom the timeline to roughly `ms` milliseconds wide around the live edge (Ctrl+wheel anchored at the cursor).
export async function zoomTo(H, ms, x, y) {
  for (let i = 0; i < 40; i++) {
    const s = await H.sel(); const w = (s.view[1] - s.view[0]) / 1e6;
    if (Math.abs(Math.log(w / ms)) < 0.35) break;
    await H.page.mouse.move(x, y); await H.page.keyboard.down("Control"); await H.page.mouse.wheel(0, w > ms ? -100 : 100); await H.page.keyboard.up("Control");
    await H.sleep(120);
  }
}
