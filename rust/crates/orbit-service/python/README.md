# orbit-profiler

Orbit is a low-overhead sampling and instrumentation profiler with a live web
viewer. This package installs the whole thing with one command.

```console
$ pip install orbit-profiler
$ orbit-service
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

## What is in the wheel, and what is not

Sampling, scheduling, manual instrumentation through the API (C, C++, Rust,
Python) and both hook engines are in the binary. Function hooks also need
`liborbit_frida_agent` and `orbit-frida-helper` in the same directory as
`orbit-service`. The wheel includes them (see `tools/release/build_wheels.sh`).
The console script marks `orbit-frida-helper` executable, because a wheel
does not keep that bit. NVIDIA GPU lanes need `orbit-gpu-helper` beside the
service; the release archive has it, this wheel does not.

On Linux, sampling needs `perf_event_paranoid` at 2 or below, and the
scheduler needs 0 or below. Root, `CAP_PERFMON`, or `CAP_SYS_ADMIN` bypass
that sysctl. Frida hooks do not need root. Auto-profiling uses the kernel
hook engine and needs root or `CAP_SYS_ADMIN`. The `curl … | sh` installer
and the release archives are in [docs/releasing.md](../../../../docs/releasing.md).
