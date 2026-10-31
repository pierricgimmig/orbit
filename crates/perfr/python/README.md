# perfr

perfr profiles a program that is already running. The service runs on Linux
and macOS, and the viewer is a web page. This package installs the build for
this machine with one command.

```console
$ pip install perfr
$ perfr
# then open http://127.0.0.1:44766/ in a browser
```

The wheel carries a prebuilt, self-contained `perfr` binary (the
capture service, which also serves the viewer), the `libperfr_api`
instrumentation library and `perfr.h`, and depends on
[`perfr-api`](https://pypi.org/project/perfr-api/) so `import perfr_api` works
for instrumenting Python.

- `perfr ...` runs the bundled binary (also `python -m perfr`).
- `perfr.binary_path()`, `.library_path()`, `.header_path()` locate
  the bundled files, e.g. to point a build system at `perfr.h`.

## What is in the wheel, and what is not

The Linux binary samples, records the scheduler, and hooks functions. The
macOS binary records manual scopes and Frida function hooks; sampling, the
scheduler, live disassembly, and auto-profiling are not in that build.
Manual instrumentation through the API (C, C++, Rust, Python) is in both.
Function hooks need `libperfr_frida_agent` and `perfr-frida-helper` in the
same directory as `perfr`. The wheel includes them (see
`tools/release/build_wheels.sh`).
The console script marks `perfr-frida-helper` executable, because a wheel
does not keep that bit. NVIDIA GPU lanes need `perfr-gpu-helper` beside the
service; the release archive has it, this wheel does not.

Frida hooks do not need root on either OS. On macOS, attaching needs a
task port. On Linux, sampling needs `perf_event_paranoid` at 2 or below,
and the scheduler needs 0 or below. Root, `CAP_PERFMON`, or `CAP_SYS_ADMIN`
bypass that sysctl. Auto-profiling uses the kernel hook engine and needs
root or `CAP_SYS_ADMIN`. The `curl … | sh` installer
and the release archives are in [docs/releasing.md](../../../../docs/releasing.md).
