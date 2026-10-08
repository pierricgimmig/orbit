// Live table: count/total/avg/min/max/std dev per scope, updated as events arrive; click a row for its duration histogram.
import { reportOpen } from "./_common.mjs";
export async function prep(H) {
  await H.key("Escape"); await reportOpen(H, true); await H.clickLabel("Live", 1); await H.sleep(800);
  // The report's scroll offset carries over from the previous tab (06 scrolls the bottom-up tree), so scroll the
  // table back to the top over its body (not over sort:count, which may itself be scrolled off).
  const f = await H.waitFor("report_filter");
  await H.glide(f[0] + f[2] / 2, f[1] + 300, 1);
  for (let i = 0; i < 4; i++) { await H.wheel(15, -200, 30); await H.sleep(400);
    const r = await H.find("sort:count"); if (r && r[1] > f[1]) break; }
}
// Make a Live row visible before clicking it. The table's visible bottom is not exposed (rows below it still have
// labels, hidden under the histogram area), so keep the row in the upper part of the table.
async function reveal(H, label) {
  const f = await H.find("report_filter");
  for (let i = 0; i < 12; i++) {
    const r = await H.find(label); if (!r) throw new Error(`no label ${label}`);
    if (r[1] > f[1] + 70 && r[1] + r[3] < f[1] + 420) return;
    await H.glide(f[0] + f[2] / 2, f[1] + 300, 300); await H.wheel(1, r[1] < f[1] + 70 ? -100 : 100, 60); await H.sleep(300);
  }
}
export default async function (H, ctx) {
  await H.sleep(400);
  // the Live table re-sorts by activity every second; sorting by a column freezes the order so rows can be clicked
  await reveal(H, "sort:total"); await H.clickLabel("sort:total", 700); await H.sleep(1200);
  ctx.mark("hero_in");
  await reveal(H, "live:simulate_physics"); await H.clickLabel("live:simulate_physics", 800); await H.sleep(2200);   // histogram + timeline highlight
  const h = await H.waitFor("report_splitter");
  await H.glide(H.W - 300, H.H - 120, 800); await H.sleep(1500);          // over the histogram
  ctx.mark("hero_out");
  await ctx.still("still-live-table-histogram");
  await reveal(H, "live:render_scene"); await H.clickLabel("live:render_scene", 800); await H.sleep(2000);
  await reveal(H, "live:update_ai"); await H.clickLabel("live:update_ai", 600); await H.sleep(2000);
  await reveal(H, "sort:avg"); await H.clickLabel("sort:avg", 600); await H.sleep(1500);
  await reveal(H, "sort:count"); await H.clickLabel("sort:count", 600); await H.sleep(1200);
}
// clicking a Live row filters the timeline to that scope: clear it for the next clips
export async function after(H) { await H.key("Escape"); await H.sleep(300); await H.key("Escape"); await H.sleep(300); }
