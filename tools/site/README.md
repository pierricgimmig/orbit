<!-- Copyright (c) 2026 The Orbit Authors. All rights reserved.
     Use of this source code is governed by a BSD-style license that can be
     found in the LICENSE file. -->

# The project web site

`build_site.py` assembles a static directory: the viewer pack, one capture
the front page opens with no service behind it, the manual (rendered from
`docs/manual/*.md`), the blog, the screenshots and the latest e2e report.
`serve.py` serves it with the cross-origin isolation headers the viewer's
worker pool needs. Standard library only.

```
python3 tools/site/build_site.py                       # captures Box3D for the front page
python3 tools/site/build_site.py --bundle my.orbit.zip # or a saved capture
python3 tools/site/build_site.py --stream my.orbit.stream
python3 tools/site/serve.py --dir site --port 8081     # http://<lan address>:8081/
```

## The embedded capture

The viewer can open a `.orbit.stream` file instead of connecting to a
service: `viewer/index.html?capture=<url>`. The file is exactly the frames a
connecting viewer receives (`GET /api/capture/export?format=stream`): the
Hello, every interned name, thread and process names, the status, the
events in packed batches, and a `CaptureFinished` so the view fits. The
viewer decodes it with the code path it already has for the socket, so the
static mode added no format and no dependency.

What works with no service: the timeline, focus and selection, the scope
menu, the Live tab and its histogram, the Self pane, and the sampling
report -- Flat, Top-down, Bottom-up, Flame and the scope-scoped report
are computed in the viewer from the sampled frames the stream carries.
What needs a service and is hidden or inert on the site: Record, Demo,
Open, Clear, Save, the process row, Modules, and hooking.

## Hosting

The output is plain files. Links and asset URLs are relative, so the same
tree works at a domain root and under a project-site prefix (`/orbit/` on
GitHub Pages). Nothing is baked as `/manual/...` or `/viewer/...`, and there
is no `CNAME`: a custom domain can be pointed at Pages later without a
rebuild.

The viewer's worker pool needs `Cross-Origin-Opener-Policy: same-origin`
and `Cross-Origin-Embedder-Policy: require-corp`. `serve.py` sends them.
GitHub Pages cannot, so every document loads `coi-serviceworker.js`, which
adds the headers and reloads once. If the worker cannot install, the viewer
stays on its sequential lane walk and still draws the capture. A page that
is already isolated does not register the worker.

`.github/workflows/pages.yml` builds the viewer pack, then this script with
the saved stream `docs/blog/captures/ghosts-on.orbit.stream` (CI has no
process to capture), and deploys the `site/` tree to Pages.

`site/` is ignored by git; the inputs are.


## Served by orbit-service (dev only)

`orbit-service` serves a built site at `/site`, straight from disk -- a
dev convenience while there is no public site, so nothing is baked into
the binary. Build the site for the service and run the service from the
repo root:

```
python3 tools/site/build_site.py --service
orbit-service --serve 44766     # run from the repo root, where ./site is
```

`--service` builds a site that carries no viewer copy of its own: its
embeds point at the service's viewer at `/`, and the capture files under
`site/` are the only large payload. The service finds `./site` in the
working directory, or `ORBIT_SITE_DIR` if set. The viewer's More menu has
a "Website & docs" link to `/site`. Nothing is added to the binary, and
`/site` shows a short note when no site is found.
