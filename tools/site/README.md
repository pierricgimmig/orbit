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

The output is plain files. Any static host works; a host that cannot set
`Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` (GitHub Pages, for one) still
works, with the viewer's lane walk running single-threaded.

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
