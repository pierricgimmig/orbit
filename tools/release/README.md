# Release tooling

## Python wheels

`build_wheels.sh` builds the two pip packages for one or more platforms:

| Package | Wheel carries | Command it adds |
|---|---|---|
| `orbit-api` | prebuilt `liborbit_api` (glibc 2.17 on Linux) | — (`import orbit_api`) |
| `orbit-profiler` | `orbit-service` binary + `liborbit_api` + `orbit.h`, depends on `orbit-api` | `orbit-service` |

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
zig cross-links every target, so a single Linux host can build the Linux wheels;
the macOS wheels build on a macOS runner (or cross from Linux with zig). Wheels
are stripped and tagged `py3-none-<platform>` — one wheel serves every Python 3.

Local check:

```bash
python3 -m venv /tmp/v
/tmp/v/bin/pip install --no-index --find-links dist/wheels orbit-profiler
/tmp/v/bin/python -c "import orbit_api as o; print(o.init(), o.library_path())"
/tmp/v/bin/orbit-service --help
```

CI (`.github/workflows/python-wheels.yml`) runs the matrix on a `v1.*` tag and
publishes to PyPI via Trusted Publishing — configure the publishers for
`orbit-api` and `orbit-profiler` on PyPI once before the first tagged release.
