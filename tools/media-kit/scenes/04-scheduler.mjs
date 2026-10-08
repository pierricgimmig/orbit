// Scheduler: one lane per core, game_loop's slices, core tooltips, thread focus greys the other threads, thread states.
import { reportOpen, threadIds, zoomTo } from "./_common.mjs";
export async function prep(H, ctx) {
  await reportOpen(H, false); await H.key("Escape");
  const [pid, tid] = await threadIds(H); ctx.state.tid = [pid, tid];
  await zoomTo(H, 120, H.W * 0.6, await H.scopeY(pid, tid, 0));
}
export default async function (H, ctx) {
  const [pid, tid] = ctx.state.tid;
  const sch = await H.waitFor("row:scheduler");                 // FRAGILE: core lanes have no labels; lane k sits
  const laneY = (k) => sch[1] + sch[3] + 6 + 13 * k;            // under the "Scheduler (N cores)" header, ~13 px apart.
  await H.sleep(500);
  await H.glide(H.W * 0.5, laneY(0), 700);
  for (let k = 0; k < 6; k++) { await H.glide(H.W * (0.5 + 0.03 * k), laneY(k), 380); await H.sleep(420); }
  ctx.mark("hero_in");
  const th = await H.waitFor(`row:thread:${pid}:${tid}`);
  await H.click(th[0] + 60, th[1] + 8, 800); await H.sleep(1600);   // focus game_loop: scheduler greys the rest + chip
  for (let k = 0; k < 6; k++) { await H.glide(H.W * (0.62 + 0.02 * k), laneY(k), 300); await H.sleep(300); }
  const sb = await H.waitFor(`sample_bar:${pid}:${tid}`);
  await H.glide(H.W * 0.55, sb[1] - 9, 700); await H.sleep(1300);   // thread-state strip tooltip (Running / Sleeping)
  await H.glide(H.W * 0.68, sb[1] - 9, 700); await H.sleep(1300);
  ctx.mark("hero_out");
  await ctx.still("still-scheduler-focus");
  await H.key("Escape"); await H.sleep(600);
}
