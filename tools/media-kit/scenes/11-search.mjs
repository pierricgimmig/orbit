// Scope search: typing in the "scope" box keeps matching scopes lit and greys out the rest; Escape clears.
import { reportOpen, threadIds, zoomTo } from "./_common.mjs";
export async function prep(H, ctx) {
  await H.key("Escape"); await reportOpen(H, false);
  const [pid, tid] = await threadIds(H); ctx.state.tid = [pid, tid];
  await zoomTo(H, 250, H.W * 0.6, await H.scopeY(pid, tid, 0));
}
export default async function (H, ctx) {
  const t = await H.waitFor("filter:tracks");        // FRAGILE: the scope search box has no label; it is the box
  const sx = t[0] - 90, sy = t[1] + t[3] / 2;        // just left of the tracks box ("filter:tracks").
  await H.sleep(500);
  await H.click(sx, sy, 900); await H.sleep(300);
  ctx.mark("hero_in");
  await H.type("draw_mesh", 110); await H.sleep(2200);
  const [pid, tid] = ctx.state.tid;
  await H.glide(H.W * 0.5, await H.scopeY(pid, tid, 2), 800); await H.sleep(1500);
  ctx.mark("hero_out");
  await H.click(sx, sy, 700); await H.page.keyboard.press("Control+A"); await H.type("simulate", 110); await H.sleep(2200);
  await H.key("Escape"); await H.sleep(1200);
}
