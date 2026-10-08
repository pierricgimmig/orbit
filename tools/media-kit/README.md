# orbit-media-kit

Lives at `tools/media-kit/` in the Orbit repo. Regenerate the landing-page
clips for a newer build, then publish them into the site:

```bash
cd tools/media-kit
./generate.sh --ref <git-ref> --out /tmp/orbit-media
python3 publish_site_media.py --from /tmp/orbit-media
```

`publish_site_media.py` writes H.264 MP4 and JPEG posters to `tools/site/media/`
(max width 1280, no audio, no GIF, no WebM, no `raw/`). It also applies the
per-clip crop rects and trims in that script, so a regenerated 1600×1000 set
is framed the same way. Rebuild with `python3 tools/site/build_site.py`.
See `tools/site/README.md`.

A single command builds `orbit-service` (with the viewer embedded) from a git ref, boots a Debian VM under QEMU **TCG**,
runs a dummy `game_loop` workload plus `orbit-service` inside it, drives one headful Chrome with Playwright through
data-driven scenes, records each scene losslessly with ffmpeg x11grab, and converts the recordings to MP4/WebM/GIF/PNG,
a hero montage, stills and `MANIFEST.md`.

```bash
cd tools/media-kit
./generate.sh --ref <git-ref> --out <dir>             # e.g. --ref da8aa05c --out /tmp/orbit-media
# long run (about 35-45 min on this box), detached so a dropped shell does not kill it:
setsid nohup ./generate.sh --ref da8aa05c --out /workspace/orbit-media > /dev/null 2>&1 < /dev/null &
tail -f /workspace/orbit-media/raw/generate.log
```

## Prerequisites (`steps/00-prereqs.sh`, run by generate.sh)

* Debian 13 box with sudo. apt packages: `qemu-system-x86 qemu-utils openssh-client xvfb x11-utils ffmpeg xz-utils curl python3 gcc iproute2`
  (+ `genisoimage` only for `steps/vm-create.sh`). The script only runs apt when something is missing. `apt-get update`
  and `install` are each retried 3 times. A failed `update` is just a warning (deb.debian.org sometimes answers
  `500 Internal Server Error`), and nothing fails while all packages are already installed.
* `gifski` 1.34.0 (installed from the GitHub release tarball if missing).
* Google Chrome at `/usr/bin/google-chrome` (override with `CHROME=`), node >= 18. `npm ci` in `node/` installs `playwright-core` 1.59.1
  (it uses the system Chrome, so it downloads no browsers).
* Build toolchains are only needed for `--ref` builds: rustup with `nightly-2025-11-15` + `wasm32-unknown-unknown`, `wasm-bindgen-cli 0.2.100`,
  toolchain 1.88 + the `x86_64-unknown-linux-musl` target (rustup installs what `rust-toolchain` asks for).
  `--binary <orbit-service>` skips the build.
* Source: `ORBIT_REPO` (default `/workspace/orbit-autoinstr`, a clone of pierricgimmig/orbit). `steps/10-build.sh` works in a private
  `git clone --shared` under `.cache/src`, so **the repo's working tree is never touched**. It restores `viewer-dist` in the cache clone after the
  build, and it only ever fetches (read-only) when a ref is not found locally. Binaries are cached as `.cache/bin/orbit-service-<sha8>`.

## The VM (`lib/config.sh`: `VM_DIR`, default `/workspace/autoinstr-test/vm`)

* `disk.qcow2`: an overlay on the Debian 13 genericcloud image, user `tester` with passwordless sudo, ssh key `vmkey`.
* `steps/20-vm.sh up|down|status` boots it with **`-accel tcg,thread=multi -cpu max -smp 6 -m 2048`**. Never use KVM: nested KVM crashes this
  box's kernel. It forwards `127.0.0.1:2222 -> 22` (ssh) and `127.0.0.1:44900 -> 44766` (viewer/API). `down` powers off over ssh, then kills the qemu pid.
* Recreate the VM from scratch with `steps/vm-create.sh` (needs genisoimage). It downloads `debian-13-genericcloud-amd64.qcow2`, generates
  `vmkey`, builds a cloud-init seed from `vm/user-data` + `vm/meta-data`, creates the 8 GB overlay, runs the first boot until
  `cloud-init status --wait` returns, and powers off.
* `steps/30-target.sh start <bin>` compiles `dummy/game_loop.c` **on the box** (the guest has no gcc; both use glibc 2.41) with
  `-O0 -g -fno-omit-frame-pointer -fno-inline -fdebug-prefix-map`, so the Code tab finds `game_loop.c` next to the service. It copies the
  service, binary and source to `/home/tester/media`, starts `sudo orbit-service --serve 44766 --log-dir logs` detached, waits for
  `/api/status`, and starts `./game_loop 40` (phase 2 of its workload begins after 40 s). `stop` stops both and tars the guest logs into `<out>/raw/guest-logs`.

## What generate.sh does

`05-stale` (kill leftovers of an aborted run via pidfiles) → `prereqs` → `build` → clean old outputs → `vm up` → `target start` →
`40-record.mjs` (all clips in **one** viewer page) → save `raw/status-final.json` → `target stop` → `vm down` → `50-convert.py` → `60-manifest.py`.

Options: `--binary PATH`, `--only id1,id2`, `--skip-existing`, `--steps prereqs,build,vm,target,record,convert,manifest,down`, `--keep-vm`, `--no-clean`.

* **Aborted runs.** Every long-lived process has a pidfile in `.cache/run/`: `generate.pid`, `qemu.pid`, `recorder.pid` (node), `chrome.pid`
  (Playwright's browser) and `xvfb.pid`. A new run starts with `steps/05-stale.sh`, which kills the recorder, Chrome (and any leftover
  `playwright_chromiumdev_profile` processes) and Xvfb, and powers off and kills a stale VM. It also warns if ports 2222/44900 are held by
  something else. When generate.sh fails or receives TERM/INT/HUP, its trap runs the same cleanup. A second concurrent generate.sh refuses to start.
* **Resume.** `--skip-existing` keeps every clip already in `<out>/raw/clips.json` without an error and whose `.mkv` exists. It records the
  rest and converts everything. It implies `--no-clean`. Clip 01 normally starts the capture from the UI, so when 01 is skipped (or not in
  `--only`) and no capture is running, the recorder starts the same capture through the API (`POST /api/symbols/load`, then
  `POST /api/capture/start` with uprobes, auto-profile, 1 kHz DWARF sampling, CSW and thread states) and reloads the page.
  Re-record one clip with `./generate.sh --ref X --out D --only 07-live-table` (the VM boots fresh, so the capture is younger than in a full run; scenes that need history use `warmup_s`).
* Re-convert without recording: `./generate.sh --out D --steps convert,manifest` (or `python3 steps/50-convert.py --out D [--only ids]`). `--only` skips the hero montage; a plain `--steps convert,manifest` rebuilds it.

## Outputs (`--out`)

* `NN-name.{mp4,webm,gif,png}`. MP4 is H.264 High, yuv420p, crf 18, +faststart, 30 fps. WebM is VP9 crf 32. The GIF is made with gifski, starting
  at 960 px (never wider than the source) / 15 fps (`GIF_FPS`) / q90 and stepping down quality, fps and width until it is ≤ 4 MB. The poster PNG is taken near the scene's end (`hero_out`).
* `00-hero.{mp4,webm,gif,png}`: the `hero_in..hero_out` window (≤ 3.6 s each) of every `hero: true` clip, joined with 0.35 s crossfades. GIF ≤ 8 MB.
* `stills/*.png`: full-resolution (1600x1000) frames grabbed by the scenes (`ctx.still(name)`).
* `MANIFEST.md`: file table (size, duration, resolution, speed, caption), stills, feature coverage, what is not captured, scene errors.
* `raw/`: lossless `.mkv` recordings, `clips.json` (rec length, marks, stills, errors), `convert.json`, `record.log`, `generate.log`, `commands.txt` (every generate.sh run on this directory, with aborted ones marked; listed in MANIFEST.md), `guest-logs/`.

## Adding or editing a clip

1. Add an entry to `clips.mjs` (`id`, `scene`, `title`, `caption`, optional `warmup_s`, `max_s`, `crop`, `speed`, `hero`). Order matters:
   all clips share one page and one capture, so a scene should leave the UI as it found it (panels closed, Follow on). Use `after()` for that.
2. Write `scenes/<scene>.mjs`:
   ```js
   import { reportOpen, zoomTo } from "./_common.mjs";
   export async function prep(H, ctx) { await reportOpen(H, false); await zoomTo(H, 500); }   // before recording starts
   export default async function (H, ctx) {                                                    // recorded
     await H.clickLabel("More"); ctx.mark("hero_in"); /* ... */ await ctx.still("still-name"); ctx.mark("hero_out");
   }
   export async function after(H, ctx) { await H.key("Escape"); }                              // after recording
   export const region = { w: 390, h: 800 };   // optional: record only this CSS-px region (top-left)
   ```
   `H` is in `lib/harness.mjs`. Pointer helpers are `glide/click/dblclick/drag/wheel` (eased, time-based, with a drawn cursor and click
   ripple), plus keyboard (`hold`, `type`, `key`). Locating helpers are `find(label)`, `center(label)`, `waitFor(label)`, `sel()` and
   `waitSel(pred)`, plus `scopeY(pid,tid,depth)`. Pill state is read with `ensurePill(label,on)` and `pillOn(label)`.
3. Iterate without re-running everything: `node steps/40-record.mjs --out /tmp/x --serve` keeps the browser open (requires the VM and target up:
   `steps/20-vm.sh up && steps/30-target.sh start $(steps/10-build.sh <ref> | tail -1)`). Then run `./tools-dev.sh clip <id>` (records one clip)
   or `./tools-dev.sh -e 'await H.clickLabel("More")'`. Watch with `x11vnc -display :77` if needed. Finish with
   `./generate.sh --ref <ref> --out <dir> --only <id>`, or with `--skip-existing`.

## How elements are located (selectors)

The viewer paints everything on a `<canvas>`, so there are no DOM selectors. The kit uses the viewer's own test hooks:

* `window.__orbit_ui`: a JSON `[[label, x, y, w, h], ...]` in CSS px of the painted, labelled widgets. Examples are `Record`, `Stop`, `Open`,
  `Settings`, `Process`, `visible-process:<pid>`, `symbols-status:…`, `More`, `Report`, `Follow:on|off`, pills (`CSW`, `Uprobes`, `Auto`, `Functions`, …),
  report tabs (`Flame`, `Top-down`, …), `report_filter`, `sort:*`, `tree:<fn>`, `live:<fn>`, `hook:<fn>`, `menu:hook`, `menu:disassemble`, and rows
  (`row:scheduler`, `row:thread:<pid>:<tid>`, `sample_bar:<pid>:<tid>`).
* `window.__orbit_sel`: JSON state such as `recording`, `capture_elapsed`, `view` (`t0`/`t1`), `report_open`, `tab`, `code`, `selected_pid`.

Prefer these over coordinates. Scenes wait on state (`waitSel(s => s.tab === "Code")`) instead of sleeping where they can.
Some elements have no hook, so the scenes derive their positions from a neighbouring label. These are the fragile spots, marked `FRAGILE` in `scenes/`.
They are the **UI elements that need a stable hook (suggested `__orbit_ui` label / data-testid)**:

| element | used by | current workaround | suggested label |
|---|---|---|---|
| Auto-profile status line and "Hooked N" text in Settings | 03 | `Functions` pill x −90 / +150 / +260 | `auto-status`, `hooked-count` |
| Settings close (×), sample period field | 01, 03 | gear toggle | `settings:close`, `settings:sample-period` |
| Pill on/off state | 01 | x11grab luminance of the pill | include `on:true` in the entry or `__orbit_sel.settings{}` |
| Follow state | all | read the `Follow:on|off` label inside the More menu | `__orbit_sel.follow` |
| Timeline scopes (hit test) | 02, 08 | `sample_bar` bottom + 9 + 21.5·depth | `scope:<tid>:<depth>:<name>` for visible scopes, or a `__orbit_hit(x,y)` |
| Scheduler core lanes | 04 | `row:scheduler` bottom + 6 + 13·core | `row:core:<n>` |
| Thread-state strip | 04 | `sample_bar` y − 9 | `thread_state:<pid>:<tid>` |
| Process expand chevron vs drag handle | 09 | `row:thread` x + 12 | `chevron:<pid>` |
| Flame graph bars | 05, 08, 13 | rows at `report_filter` bottom + 30 + 18.3·k, x as a fraction of the width | `flame:<depth>:<fn>` |
| Code tab Source / Disassembly / Both | 10 | `report_filter` bottom + 17, x + 22/79/130 | `code-mode:source|disasm|both` |
| More menu: Color scheme submenu and theme items, Compact tracks, Fit capture, Rect selection, UI knobs | 12 | `Inspector` y − 21, items at `Inspector` x − 75 + 21·k | `menu:color-scheme`, `theme:<name>`, … |
| Scope search box | 11 | `filter:tracks` x − 90 | `scope_search` |
| Live histogram; visible (clip) rect of scrolled tables | 07 | keep rows in the upper part of the table | `live-histogram`, a clip rect per scroll area |
| Process rows in the picker (names) | 01 | `visible-process:<pid>` (the pid comes from `/api/processes`) | `process-name:<name>` |
| Tooltip text | 02, 04 | none (only visual) | `__orbit_sel.tooltip` |

## Caveats

* **TCG only**, 6 vCPU / 2 GB. The workload sees roughly 730–780 samples/s at a 1 kHz setting.
* **One viewer page at a time.** Each connected viewer adds 0.4–0.85 GB to orbit-service's RSS in a 2 GB VM. The recorder uses a single page
  for all clips (only `goto` reloads). Do not open the viewer elsewhere during a run.
* **Frame rate.** Chrome renders the WASM canvas with SwiftShader (no GPU), so the canvas updates at about 8–12 fps while the box is busy
  running the TCG VM. Recordings are captured at 30 fps and the clips stay smooth for the cursor, but timeline motion is steppy. Conversion speeds
  long clips up to 1.75x (`MAX_SPEED`) (target 15 s). On a machine with a GPU, add `CHROME_EXTRA="--use-gl=…"` and drop `--use-angle=swiftshader` in `lib/harness.mjs`.
* **No CDP screenshots while recording.** `Page.captureScreenshot` makes headful Chrome flash white or black. Stills and pill checks use x11grab instead.
* Chrome's fullscreen window is screen − 1 px, so Xvfb runs at `(W+1)x(H+1)`. The recording is 1600x1000 device px (1280x800 CSS at DPR 1.25).
* Keyboard shortcuts (W/A/S/D, Space, R, Escape) go to text boxes when the scope search or report filter has focus. Scenes press Escape first.
* Do **not** press Home (fit the whole capture) on a long capture: on da8aa05c, after ~11 min with a full 8 M-event ring, the page stalled for
  75 s and then crashed with a wasm `unreachable` trap. Scenes zoom to fixed widths (`zoomTo`).
* The Chrome-trace scene (14) runs last. Opening a file **stops the live capture** and merges the trace into the timeline, so the scene stops and
  clears first. Its trace (`assets/make_chrome_trace.py`) uses pid 4242: a trace pid equal to a live pid takes the live process's name.
* The `Frida` pill shows as selected on a fresh viewer (known). Scene 01 explicitly switches to `Uprobes`.
* The report keeps its scroll offset across tabs, and the Live table re-sorts by activity every second. Scene 07 sorts by a
  column first and scrolls each row into the upper part of the table before clicking it. Labels of rows hidden under the
  histogram area are still reported, because `__orbit_ui` carries no clip rect.
* The real pointer is hidden with `cursor: none` (Xvfb paints a software cursor into the framebuffer). The drawn overlay
  is the only cursor, and stills hide it and wait two animation frames + 600 ms (the busy canvas delays compositor frames).
* Coordinates derived from labels (table above) break if the layout changes. Run `./tools-dev.sh clip <id>` after viewer UI changes.

## Files

```
generate.sh            orchestrator (see above)
clips.mjs              data-driven clip list (ids, scenes, captions, crop/speed/hero)
scenes/*.mjs           one Playwright scenario per clip; _common.mjs = shared helpers (report/settings/follow/zoomTo, API)
lib/config.sh          paths, ports, VM size, recording/GIF settings (env-overridable)
lib/harness.mjs        Chrome launch (Xvfb, kiosk, DPR), cursor overlay, label lookup, eased input, x11grab recording/stills
steps/00-prereqs.sh    apt/gifski/node checks (tolerates apt mirror errors)
steps/05-stale.sh      kill leftovers of an aborted run (pidfiles in .cache/run)
steps/10-build.sh      build wasm viewer + musl orbit-service from a git ref in .cache/src (cached per sha)
steps/20-vm.sh         QEMU TCG VM up/down/status
steps/30-target.sh     copy + start/stop orbit-service and game_loop in the VM
steps/40-record.mjs    record all clips in one page (--only, --skip-existing, --serve)
steps/50-convert.py    mkv -> mp4/webm/gif/png, hero montage
steps/60-manifest.py   MANIFEST.md
steps/vm-create.sh     recreate the VM image from the Debian cloud image
vm/user-data, vm/meta-data   cloud-init seed used by vm-create.sh
dummy/game_loop.c      the profiled workload
assets/make_chrome_trace.py  deterministic demo Chrome trace for clip 14
tools-dev.sh           drive a --serve recorder (clip <id> | -e '<js>')
node/                  playwright-core (package.json, package-lock.json)
```
