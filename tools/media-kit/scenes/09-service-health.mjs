// The service profiles itself: orbit-service's own thread scopes and lanes (perf ring fill %, scope ring fill,
// records lost, viewer ring fill, events per pass, service cpu %, service rss MiB), read with the cursor line.
import { reportOpen, api, zoomTo } from "./_common.mjs";
const expanded = async (H, svc) => (await H.labels()).some((l) => l.startsWith(`row:thread:${svc}:`));
export async function prep(H, ctx) {
  await H.key("Escape"); await reportOpen(H, false);
  // ~20 s window (Home = the whole capture: very slow with software GL and it crashed the viewer once, see README)
  await zoomTo(H, 20000, H.W * 0.8, 200);
  ctx.state.svc = (await api("/api/status")).service_pid;
  if (await expanded(H, ctx.state.svc)) { const r = await H.waitFor(`row:process:${ctx.state.svc}`); await H.click(r[0] + 12, r[1] + r[3] / 2, 1); await H.sleep(1500); }
  await H.glide(H.W * 0.5, H.H * 0.3, 1);
}
export default async function (H, ctx) {
  await H.sleep(500);
  const r = await H.waitFor(`row:process:${ctx.state.svc}`);
  await H.click(r[0] + 12, r[1] + r[3] / 2, 900);   // FRAGILE: the chevron is ~12 px in (the drag handle is at ~5 px)
  await H.sleep(2500);
  ctx.mark("hero_in");
  for (const fx of [0.35, 0.5, 0.65, 0.8, 0.93]) { await H.glide(H.W * fx, H.H * (0.55 + 0.25 * fx), 800); await H.sleep(600); }   // cursor-line readouts
  ctx.mark("hero_out");
  await ctx.still("still-service-health");
  await H.sleep(500);
}
export async function after(H, ctx) {   // fold it again so later clips draw fewer tracks
  if (await expanded(H, ctx.state.svc)) { const r = await H.waitFor(`row:process:${ctx.state.svc}`); await H.click(r[0] + 12, r[1] + r[3] / 2, 1); await H.sleep(800); }
}
