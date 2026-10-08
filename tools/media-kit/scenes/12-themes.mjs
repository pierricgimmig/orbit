// Colour schemes: More > Color scheme recolours chrome, scopes and thread states (Orbit, Dracula, Nord,
// Gruvbox, Solarized, Solarized Light, Gruvbox Light).
import { reportOpen, threadIds, zoomTo } from "./_common.mjs";
export async function prep(H, ctx) {
  await H.key("Escape"); await reportOpen(H, true); await H.clickLabel("Flame", 1); await H.sleep(800);
  const [pid, tid] = await threadIds(H); await zoomTo(H, 120, 400, await H.scopeY(pid, tid, 0));
}
async function pick(H, k, ms) {   // FRAGILE: menu entries have no labels. "Color scheme" is the row above "Inspector";
  await H.clickLabel("More", ms); await H.sleep(500);   // its submenu opens to the left, one row (~21 px) per scheme.
  const i = await H.waitFor("Inspector"); const cy = i[1] + i[3] / 2 - 21;
  await H.glide(i[0] + 40, cy, 400); await H.sleep(700);
  await H.glide(i[0] - 30, cy, 250); await H.click(i[0] - 75, cy + 21 * k, 350); await H.sleep(300);
}
export default async function (H, ctx) {
  await H.sleep(400);
  ctx.mark("hero_in");
  for (const k of [1, 5, 2]) { await pick(H, k, 700); await H.glide(H.W * 0.45, H.H * 0.3, 500); await H.sleep(1500); }   // Dracula, Solarized Light, Nord
  ctx.mark("hero_out");
  await pick(H, 0, 700); await H.sleep(1000);     // back to Orbit
}
export async function after(H) { await H.key("Escape"); }
