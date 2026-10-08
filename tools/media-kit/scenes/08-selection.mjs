// Time-range selection: right-drag selects every thread's samples in a window and the report follows it;
// left-drag on a thread's sample bar selects that thread only.
import { reportOpen, threadIds, zoomTo } from "./_common.mjs";
export async function prep(H, ctx) {
  await H.key("Escape"); await reportOpen(H, true); await H.clickLabel("Flame", 1); await H.sleep(500);
  const [pid, tid] = await threadIds(H); ctx.state.tid = [pid, tid];
  await zoomTo(H, 1500, 500, await H.scopeY(pid, tid, 0));
}
export default async function (H, ctx) {
  const [pid, tid] = ctx.state.tid; const sb = await H.waitFor(`sample_bar:${pid}:${tid}`);
  await H.sleep(500);
  ctx.mark("hero_in");
  await H.drag(sb[0] + sb[2] * 0.15, sb[1] + 120, sb[0] + sb[2] * 0.55, sb[1] + 120, 1500, "right"); await H.sleep(2200);
  await H.clickLabel("Top-down", 600); await H.sleep(1800);
  ctx.mark("hero_out");
  await ctx.still("still-selection-report");
  await H.key("Escape"); await H.sleep(800);
  await H.drag(sb[0] + sb[2] * 0.6, sb[1] + sb[3] / 2, sb[0] + sb[2] * 0.85, sb[1] + sb[3] / 2, 1300, "left"); await H.sleep(2000);
  await H.clickLabel("Flame", 600); await H.sleep(2000);
  await H.key("Escape"); await H.sleep(500);
}
