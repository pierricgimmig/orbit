// Auto-profiling: the status line counts hooked functions until "converged"; the Hooked row; Functions view.
import { setSettings } from "./_common.mjs";
export async function prep(H) { await H.glide(H.W * 0.6, H.H * 0.5, 300); }
export default async function (H, ctx) {
  await H.sleep(400);
  await setSettings(H, true, 700); await H.sleep(600);
  const f = await H.waitFor("Functions");            // FRAGILE: the Hooked row text and the status line have no labels;
  const y = f[1] + f[3] / 2;                         // they sit on the Functions pill's row (Hooked count left, status right).
  await H.glide(f[0] - 90, y, 700); await H.sleep(1200);            // "N auto-profile functions hooked"
  ctx.mark("hero_in");
  await H.glide(f[0] + f[2] + 150, y, 800); await H.sleep(3500);    // status line + its tooltip (hooked set, recent actions)
  ctx.mark("hero_out");
  await H.glide(f[0] + f[2] + 260, y, 600); await H.sleep(2500);
  await H.clickLabel("Functions", 700); await H.sleep(1500);          // Functions view: hooked column
  await setSettings(H, false, 600);
  const r = await H.find("sort:hook"); if (r) { await H.clickLabel("sort:hook", 600); await H.sleep(800); await H.clickLabel("sort:hook", 400); }
  await H.sleep(1800);
  const h = await H.find("hook:simulate_physics"); if (h) { await H.glide(h[0] + 200, h[1] + h[3] / 2, 700); await H.sleep(1500); }
}
export async function after(H, ctx) { await H.key("r"); await H.sleep(500); }
