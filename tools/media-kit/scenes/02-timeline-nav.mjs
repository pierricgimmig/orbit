// Live timeline navigation: cursor-anchored Ctrl+wheel zoom, drag pan, W/S zoom and A/D pan, scope tooltips.
import { reportOpen, setSettings, threadIds, zoomTo, follow } from "./_common.mjs";
export async function prep(H, ctx) {
  await setSettings(H, false, 1); await reportOpen(H, false); await H.key("Escape");
  await follow(H, true);
  const [pid, tid] = await threadIds(H); ctx.state.tid = [pid, tid];
  await H.glide(H.W * 0.7, await H.scopeY(pid, tid, 0), 300);
  await zoomTo(H, 4000, H.W * 0.7, await H.scopeY(pid, tid, 0));   // start from a ~4 s window at the live edge
}
export default async function (H, ctx) {
  const [pid, tid] = ctx.state.tid; const y0 = await H.scopeY(pid, tid, 0);
  await H.sleep(800);
  await H.glide(H.W * 0.62, y0 + 10, 700);
  ctx.mark("hero_in");
  await H.wheel(14, -100, 110, ["Control"]); await H.sleep(900);       // zoom in around the cursor
  await H.drag(H.W * 0.70, y0 + 120, H.W * 0.35, y0 + 120, 1300);      // drag to pan back in time (leaves Follow)
  await H.sleep(500);
  await H.hold("w", 900); await H.sleep(300);                          // W: zoom in
  ctx.mark("hero_out");
  for (const [fx, d] of [[0.42, 0], [0.47, 1], [0.52, 2], [0.58, 1]]) {   // tooltips: run_frame > dispatch > simulate_physics ...
    await H.glide(H.W * fx, await H.scopeY(pid, tid, d), 650); await H.sleep(1100);
  }
  await H.hold("d", 700); await H.sleep(300);                          // D: pan right
  await H.hold("s", 800); await H.sleep(600);                          // S: zoom out
  await ctx.still("still-timeline-nested-scopes");
}
