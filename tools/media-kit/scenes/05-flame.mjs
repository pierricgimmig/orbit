// Sampling flame graph: hover names a bar, click highlights the function on the timeline, double-click zooms.
import { reportOpen } from "./_common.mjs";
export async function prep(H) { await H.key("Escape"); await reportOpen(H, true); await H.clickLabel("Live", 1); await H.sleep(800); }
export default async function (H, ctx) {
  await H.sleep(500);
  await H.clickLabel("Flame", 700); await H.sleep(1500);
  const f = await H.waitFor("report_filter");                       // FRAGILE: flame bars have no labels; rows are
  const rowY = (k) => f[1] + f[3] + 30 + 18.3 * k;                  // ~18 px under the filter box, root (_start) first.
  const x0 = f[0], w = f[2];
  ctx.mark("hero_in");
  await H.glide(x0 + w * 0.5, rowY(5), 800); await H.sleep(900);    // run_frame
  await H.glide(x0 + w * 0.2, rowY(6), 500); await H.sleep(900);    // dispatch_game_systems
  await H.glide(x0 + w * 0.12, rowY(7), 500); await H.sleep(800);   // simulate_physics
  await H.glide(x0 + w * 0.36, rowY(7), 500); await H.sleep(800);   // update_ai
  await H.glide(x0 + w * 0.55, rowY(6), 500); await H.sleep(800);   // render_scene
  await H.click(x0 + w * 0.2, rowY(6), 600); await H.sleep(1000);   // highlight dispatch_game_systems on the timeline
  await H.dblclick(x0 + w * 0.2, rowY(6), 300); await H.sleep(1500);   // zoom the graph to that subtree
  await H.glide(x0 + w * 0.3, rowY(8), 600); await H.sleep(900);
  await H.glide(x0 + w * 0.75, rowY(7), 600); await H.sleep(900);
  ctx.mark("hero_out");
  await ctx.still("still-flame-graph");
  await H.dblclick(x0 + w * 0.5, rowY(0), 600); await H.sleep(1200); // back to the root
}
// the bar click put the function into the scope search box: clear it so later clips start clean
export async function after(H) { await H.key("Escape"); await H.sleep(300); await H.key("Escape"); await H.sleep(300); }
