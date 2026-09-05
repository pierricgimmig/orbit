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
menu's highlight, the Live tab and its histogram, the Self pane. What needs
a service and is hidden or inert on the site: Record, Demo, Open, Clear,
Save, the process row, and the sampling report tabs (Flat, Top-down,
Bottom-up, Modules, Flame), which the service computes.

## Hosting

The output is plain files. Any static host works; a host that cannot set
`Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` (GitHub Pages, for one) still
works, with the viewer's lane walk running single-threaded.

`site/` is ignored by git; the inputs are.
