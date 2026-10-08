// Open a Chrome Trace Event JSON (or Perfetto) file: Open > pick the file; it replaces the session in this page.
// Runs last: it stops the live capture and clears it first.
import path from "path"; import { fileURLToPath } from "url"; import { execFileSync } from "child_process";
import { reportOpen } from "./_common.mjs";
const KIT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export async function prep(H, ctx) {
  ctx.state.trace = "/tmp/orbit-media-demo-trace.json";
  execFileSync("python3", [path.join(KIT, "assets/make_chrome_trace.py"), ctx.state.trace]);
  await H.key("Escape"); await reportOpen(H, false);   // the timeline needs the full width
  // Stop and Clear first: opened during a live capture, the trace is merged into the live session (and its
  // pid 1 takes the live machine's pid-1 name), see README "Known issues".
  if ((await H.sel()).recording) { await H.clickLabel("Stop", 1); await H.waitSel((s) => !s.recording, 30000); await H.sleep(1500); }
  await H.clickLabel("Clear", 1); await H.sleep(2500);
}
export default async function (H, ctx) {
  await H.sleep(500);
  const chooser = H.page.waitForEvent("filechooser", { timeout: 15000 });
  await H.clickLabel("Open", 900);
  const fc = await chooser; await H.sleep(600); await fc.setFiles(ctx.state.trace);
  await H.sleep(4000);
  ctx.mark("hero_in");
  await H.glide(H.W * 0.55, H.H * 0.35, 800);
  await H.wheel(6, -100, 120, ["Control"]); await H.sleep(1200);
  for (const [fx, fy] of [[0.4, 0.22], [0.5, 0.27], [0.6, 0.45], [0.7, 0.6]]) { await H.glide(H.W * fx, H.H * fy, 600); await H.sleep(800); }
  ctx.mark("hero_out");
  await ctx.still("still-chrome-trace");
  await H.key("Home"); await H.sleep(1500);
}
