# Orbit architecture manual

Self-contained HTML manual and architecture review of this repository.

Open [`index.html`](index.html) in a browser, or serve this directory as a static site:

```
python3 -m http.server --directory docs/manual 8080
```

Relative links only. No CDN, no webfonts, no external images.

GitHub Pages: set the site source to `/docs` and open `/manual/`, or publish this folder as the site root.

The pages review the code as of the commit that added them. They do not change profiler behavior.
