// Code view: right-click a function in the call tree > "Show disassembly and source": the service disassembles
// it out of the target's module and interleaves game_loop.c source lines (Source / Disassembly / Both).
import { reportOpen } from "./_common.mjs";
export async function prep(H) { await H.key("Escape"); await reportOpen(H, true); await H.clickLabel("Top-down", 1); await H.sleep(1500); }
async function openMenu(H, label) {
  for (let i = 0; i < 3; i++) {
    const [x, y] = await H.center(label); await H.click(x, y, i ? 200 : 800, "right");
    try { await H.waitFor("menu:disassemble", { timeout: 2500 }); return; } catch { await H.key("Escape"); }
  }
  throw new Error("context menu did not open");
}
export default async function (H, ctx) {
  await H.sleep(500);
  await openMenu(H, "tree:simulate_physics"); await H.sleep(700);
  await H.glideTo("menu:hook", 400); await H.sleep(500);
  await H.clickLabel("menu:disassemble", 500);
  await H.waitSel((s) => s.tab === "Code" && s.code && !s.code.loading && s.code.instructions > 0, 20000); await H.sleep(1200);
  ctx.mark("hero_in");
  const f = await H.waitFor("report_filter");             // FRAGILE: the Source / Disassembly / Both control has no labels
  const y = f[1] + f[3] + 17;
  for (let i = 0; i < 2; i++) { await H.glide(f[0] + 140 + i * 40, y + 80 + i * 110, 600); await H.sleep(500); }
  await H.click(f[0] + 22, y, 700); await H.sleep(1500);    // Source
  ctx.mark("hero_out");
  await H.click(f[0] + 79, y, 600); await H.sleep(1500);    // Disassembly
  await H.click(f[0] + 130, y, 600); await H.sleep(1200);   // Both
  await ctx.still("still-code-view");
  await H.glide(f[0] + 300, y + 250, 600); await H.sleep(800);
}
