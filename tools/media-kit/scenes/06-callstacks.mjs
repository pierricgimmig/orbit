// Top-down and bottom-up call trees with inclusive / self / of-parent percentages; expand and collapse.
import { reportOpen } from "./_common.mjs";
export async function prep(H) { await H.key("Escape"); await reportOpen(H, true); }
export default async function (H, ctx) {
  await H.sleep(400);
  await H.clickLabel("Top-down", 700); await H.sleep(1400);
  await H.clickLabel("Collapse all", 600); await H.sleep(900);
  await H.clickLabel("Expand all", 600); await H.sleep(1200);
  ctx.mark("hero_in");
  for (const n of ["tree:main_loop", "tree:run_frame", "tree:dispatch_game_systems", "tree:simulate_physics", "tree:render_scene"]) {
    const r = await H.find(n); if (r) { await H.glide(r[0] + 40, r[1] + r[3] / 2, 450); await H.sleep(500); }
  }
  const rf = await H.find("tree:run_frame");                        // the inclusive % column of run_frame (~88%)
  if (rf) { const inc = await H.find("sort:inclusive"); if (inc) { await H.glide(inc[0] + inc[2] / 2, rf[1] + rf[3] / 2, 600); await H.sleep(1400); } }
  await ctx.still("still-top-down-tree");
  ctx.mark("hero_out");
  await H.clickLabel("Bottom-up", 700); await H.sleep(1500);
  for (const n of ["tree:burn_us", "tree:hash_mix", "tree:draw_mesh"]) { const r = await H.find(n); if (r) { await H.glide(r[0] + 40, r[1] + r[3] / 2, 450); await H.sleep(500); } }
  const bu = await H.find("tree:burn_us"); if (bu) { await H.click(bu[0] - 12, bu[1] + bu[3] / 2, 500); await H.sleep(1200); }   // expand callers
  await H.wheel(4, 100, 150); await H.sleep(1200);
}
