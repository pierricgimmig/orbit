# Release tooling

## Python wheels

`build_wheels.sh` builds the two pip packages for one or more platforms:

| Package | Wheel carries | Command it adds |
|---|---|---|
| `perfr-api` | prebuilt `libperfr_api` (glibc 2.17 on Linux) | — (`import perfr_api`) |
| `perfr` | `perfr` binary + `libperfr_api` + `perfr.h`, depends on `perfr-api` | `perfr` |

Both wheels for a platform are cut from **one** build, so the ring protocol
version matches between the library that writes scopes and the service that
reads them.

```bash
python3 -m venv .venv && . .venv/bin/activate
pip install cargo-zigbuild ziglang build wheel
tools/release/build_wheels.sh linux-x86_64 linux-aarch64 macos-arm64 macos-x86_64
# wheels land in dist/wheels/
```

Platform keys: `linux-x86_64`, `linux-aarch64`, `macos-x86_64`, `macos-arm64`.
zig cross-links every target from a Linux host; on a macOS host the mac targets
use Apple's toolchain. Wheels are stripped and tagged `py3-none-<platform>` —
one wheel serves every Python 3.

The script refuses to run if the release version drifts between
`crates/perfr-api/Cargo.toml`, the two `pyproject.toml`s, the
`perfr-api==` pin in perfr, and the two `__version__`s: bump all of
them together.

The perfr wheel includes the Frida companions
(`libperfr_frida_agent`, `perfr-frida-helper`, `licenses/`). The service
loads them from the directory that contains `perfr`. The script
builds them for the wheel target. `PERFR_COMPANIONS_DIR=<dir>` skips that
build and copies a directory already produced by `tools/frida/build.sh`.

Local check:

```bash
python3 -m venv /tmp/v
/tmp/v/bin/pip install --no-index --find-links dist/wheels perfr
/tmp/v/bin/python -c "import perfr_api as o; print(o.init(), o.library_path())"
/tmp/v/bin/perfr --help
```

CI (`.github/workflows/python-wheels.yml`) runs the matrix on a `perfr-v*`
tag (not the legacy C++ `v1.*` tags) and publishes to PyPI via Trusted
Publishing — configure the publishers for `perfr-api` and `perfr`
on PyPI once before the first tagged release. `workflow_dispatch` builds
wheels and does not publish. See [docs/releasing.md](../../docs/releasing.md).
