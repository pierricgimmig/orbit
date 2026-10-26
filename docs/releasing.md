# Cutting a Rust release

The Rust capture service is published by
[`.github/workflows/release-rust.yml`](../.github/workflows/release-rust.yml).
Nothing in that workflow runs until you push a tag or start it by hand.
This document does not publish a release.

## Tag scheme

Tags look like `orbit-v0.1.0`. An optional suffix (`orbit-v0.1.0-rc.1`) is
allowed.

Do not use `v1.*`. Those tags, and the `nightly-*` tags, are the legacy C++
Qt UI and C++ `OrbitService`. A Rust tag on that line would collide with
`git describe --match 'v1.*'` in the C++ workflow and with the old wheel
trigger.

| Tag | GitHub Release |
|---|---|
| `orbit-vX.Y.Z` | Stable. This becomes **Latest**, which is what `install.sh` downloads. |
| `orbit-vX.Y.Z-suffix` | Prerelease. It is not Latest, so the installer ignores it. |

`/releases/latest` stays empty until the first stable `orbit-vX.Y.Z`. The
three existing C++ nightlies are already prereleases, so they are not Latest.
They were not deleted or retagged.

## What the workflow builds

Each stable tag publishes three archives: a static Linux x86-64 build, a
universal macOS build (Apple silicon and Intel), and a Windows x86-64 build.
Linux arm64 is not in the release.

Linux x86-64, on `ubuntu-22.04`:

- `orbit-service`, static musl (`x86_64-unknown-linux-musl`). The WASM viewer
  in `crates/orbit-live-viewer/viewer-dist` is embedded by `orbit-live-server` at
  compile time. The job checks that the binary is statically linked.
- The files the service looks up next to its own executable
  (`std::env::current_exe()`, overridable with `ORBIT_FRIDA_AGENT` and
  `ORBIT_FRIDA_HELPER`):
  - `liborbit_frida_agent.so`
  - `orbit-frida-helper`
- `liborbit_api.so` and `orbit.h`, for manual instrumentation.
- `orbit-gpu-helper`, the optional NVIDIA helper. It is a glibc binary on
  purpose (the static service cannot `dlopen`).
- `licenses/` for the Frida devkits.

macOS, on `macos-15`, via `tools/build/build-service-macos.sh --universal`:

- One universal `orbit-service` (Apple silicon and Intel), plus
  `liborbit_frida_agent.dylib`, `orbit-frida-helper`, `liborbit_api.dylib`,
  `orbit.h`, and the Frida licenses.
- Ad-hoc signed (`codesign --sign -`), not notarized.
- Records manual scopes and Frida function hooks. No call-stack sampling,
  scheduler, live disassembly, or auto-profiling.
- No GPU helper. That binary is Linux-only.

Windows x86-64, on `windows-2022`, via `.\tools\build\build-service-windows.ps1`:

- `orbit-service.exe` with the viewer embedded, linked against the static C
  runtime (`.cargo/config.toml`), so it depends on system DLLs only;
  the script checks that with `dumpbin /dependents`.
- `orbit_frida_agent.dll` and `orbit-frida-helper.exe` beside it, for
  function hooks; `orbit_api.dll`, `orbit_api.dll.lib`, `orbit_api.lib` and
  `include\orbit.h` for manual instrumentation; `orbit_api.py`;
  `OrbitTestRust.exe`; the Frida licenses.
- Records manual scopes and Frida hooks without privilege. Call-stack
  sampling, the scheduler and thread states come from the NT Kernel Logger
  (ETW) and need an elevated prompt. No auto-profiling, no kernel hook
  engine, no GPU helper.
- Not code-signed.

Linux arm64 is not in the release. The service compiles for arm64; it is not
run in CI, and the static musl build in this workflow is x86-64 only.

The Linux and macOS archives are gzipped tars, the Windows one a zip; each
has the top-level directory `orbit/`. Beside the archives the release has
`SHA256SUMS` and a `.sha256` file per archive (`sha256sum` format: hash, two
spaces, file name). `tools/install/install.ps1` is the Windows installer and
reads the zip's `.sha256` the way `install.sh` reads the tars'.

## How to cut one

From the commit you want to ship:

```sh
git tag orbit-v0.1.0
git push origin orbit-v0.1.0
```

Pushing the tag starts the workflow. The workflow does not create the tag.
Wait until both build jobs and the publish job are green, then check
`https://github.com/pierricgimmig/orbit/releases/tag/orbit-v0.1.0`.

`workflow_dispatch` (Actions → release-rust → Run workflow) builds the same
artifacts and uploads them to the run. Leave **tag** empty: that does not
publish and does not create a tag. Set **tag** to an `orbit-v*` tag that
already exists only when you want that run to create the GitHub Release for
it (`gh release create --verify-tag`).

The Cargo version (`0.1.0` today) is independent of the git tag. The pip
wheels are not: they publish from the same `orbit-v*` tags, and their version
has to match across the files listed in
[`tools/release/README.md`](../tools/release/README.md). Pushing `orbit-v*`
also starts `.github/workflows/python-wheels.yml`, which publishes to PyPI
when the `pypi` environment's Trusted Publishing is configured. Do not push
the tag if you do not want that publish attempt.

## Installer host

[`tools/install/install.sh`](../tools/install/install.sh) downloads
`/releases/latest/download/`. The only line to change when
`orbitprofiler.com` serves the same file names is `DEFAULT_BASE` at the top
of that script. `ORBIT_INSTALL_BASE` overrides it for one run.

Until a stable `orbit-v*` release exists, the script exits with an error that
says so and points at a source build. It does not install a C++ nightly.

## Legacy C++ nightlies

[`.github/workflows/release.yml`](../.github/workflows/release.yml) no longer
has a weekly schedule. A manual run still builds the Qt UI and C++
`OrbitService`, and still uploads a **prerelease** titled as a legacy C++
nightly. The pinned release action has no `make_latest` input; `prerelease:
true` is what keeps GitHub from selecting it as Latest. Existing
`nightly-2026-09-24`, `nightly-2026-10-01`, and `nightly-2026-10-08` releases
are unchanged.
