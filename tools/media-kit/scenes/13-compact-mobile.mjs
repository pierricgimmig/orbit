// The same viewer in a phone-sized viewport (390 CSS px wide): compact layout, touch-sized controls.
import { reportOpen } from "./_common.mjs";
export const region = { w: 390, h: 800 };   // CSS px; the recorder grabs this top-left region (x DPR)
export async function prep(H, ctx) {
  await H.key("Escape"); await reportOpen(H, false);
  await H.setViewport(390, 800); await H.sleep(2500); await H.glide(200, 400, 1);
}
export default async function (H, ctx) {
  await H.sleep(800);
  ctx.mark("hero_in");
  await H.glide(250, 300, 700); await H.wheel(6, -100, 120, ["Control"]); await H.sleep(800);
  await H.drag(300, 320, 120, 320, 1000); await H.sleep(800);
  await H.glide(200, 280, 600); await H.sleep(1200);
  ctx.mark("hero_out");
  await reportOpen(H, true, 600); await H.sleep(1800);
  await H.clickLabel("Flame", 600).catch(() => {}); await H.sleep(2000);
  await H.glide(200, 450, 600); await H.sleep(1500);
  await ctx.still("still-mobile-390");
}
export async function after(H) { await reportOpen(H, false); await H.setViewport(H.W, H.H); await H.sleep(2000); }
