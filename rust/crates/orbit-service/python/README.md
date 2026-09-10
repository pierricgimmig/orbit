# orbit-profiler

Orbit is a low-overhead sampling and instrumentation profiler with a live web
viewer. This package installs the whole thing with one command.

```console
$ pip install orbit-profiler
$ orbit-service --serve 44766
# then open http://127.0.0.1:44766/ in a browser
```

The wheel carries a prebuilt, self-contained `orbit-service` binary (the
capture service, which also serves the viewer), the `liborbit_api`
instrumentation library and `orbit.h`, and depends on
[`orbit-api`](https://pypi.org/project/orbit-api/) so `import orbit_api` works
for instrumenting Python.

- `orbit-service ...` runs the bundled binary (also `python -m orbit_profiler`).
- `orbit_profiler.binary_path()`, `.library_path()`, `.header_path()` locate
  the bundled files, e.g. to point a build system at `orbit.h`.

Sampling needs `perf_event_paranoid <= -1` or `CAP_PERFMON`; dynamic
instrumentation needs `CAP_SYS_ADMIN`. See the manual for details. The pure
`curl … | sh` installer and prebuilt archives remain at the project's releases
for users who do not want Python in the loop.
