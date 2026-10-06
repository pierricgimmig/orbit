// Pick game_loop, choose Uprobes + Auto (sampling 1 kHz, scheduler on by default), Record, live streaming.
import { targetPid, setSettings } from "./_common.mjs";
export async function prep(H, ctx) {
  await H.goto(`http://127.0.0.1:${process.env.VIEWER_PORT || 44900}/`); await H.sleep(7000);
  const s = await H.sel(); if (s.recording) throw new Error("a capture is already running; start from a fresh service");
  await setSettings(H, false, 1);
  await H.glide(H.W * 0.55, H.H * 0.45, 1);
  ctx.state.pid = await targetPid();
}
export default async function (H, ctx) {
  await H.sleep(600);
  await H.clickLabel("Process", 800); await H.sleep(500);
  await H.type("game", 110); await H.sleep(600);
  await H.clickLabel(`visible-process:${ctx.state.pid}`, 600);
  await H.waitFor("symbols-status:Loaded", { prefix: true, timeout: 20000 }); await H.sleep(500);
  await setSettings(H, true, 700); await H.sleep(500);
  for (const l of ["CSW", "States", "Sample"]) { await H.glideTo(l, 380); await H.sleep(250); }   // collection options (on by default)
  await H.ensurePill("Uprobes", true, 500); await H.sleep(350);
  await H.ensurePill("Auto", true, 500); await H.sleep(700);
  ctx.mark("hero_in");
  await H.clickLabel("Record", 800);
  await H.waitSel((s) => s.recording, 20000); await H.sleep(1200);
  await setSettings(H, false, 700);
  await H.glide(H.W * 0.62, H.H * 0.3, 900);
  await H.sleep(5500);
  ctx.mark("hero_out");
}
