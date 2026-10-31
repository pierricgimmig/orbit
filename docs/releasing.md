# Cutting a Rust release

The Rust capture service is published by
[`.github/workflows/release-rust.yml`](../.github/workflows/release-rust.yml).
Nothing in that workflow runs until you push a tag or start it by hand.
This document does not publish a release.

## Tag scheme

Tags look like `perfr-v0.1.0`. An optional suffix (`perfr-v0.1.0-rc.1`) is
allowed.

Do not use `v1.*`. Those tags, and the `nightly-*` tags, are the legacy C++
Qt UI and C++ `PerfrService`. A Rust tag on that line would collide with
`git describe --match 'v1.*'` in the C++ workflow and with the old wheel
trigger.

| Tag | GitHub Release |
|---|---|
| `perfr-vX.Y.Z` | Stable. This becomes **Latest**, which is what `install.sh` downloads. |
| `perfr-vX.Y.Z-suffix` | Prerelease. It is not Latest, so the installer ignores it. |

`/releases/latest` stays empty until the first stable `perfr-vX.Y.Z`. The
three existing C++ nightlies are already prereleases, so they are not Latest.
They were not deleted or retagged.

## What the workflow builds

Each stable tag publishes three archives: a static Linux x86-64 build, a
universal macOS build (Apple silicon and Intel), and a Windows x86-64 build.
Linux arm64 is not in the release.

Linux x86-64, on `ubuntu-22.04`:

- `perfr`, static musl (`x86_64-unknown-linux-musl`). The WASM viewer
  in `crates/perfr-live-viewer/viewer-dist` is embedded by `perfr-live-server` at
  compile time. The job checks that the binary is statically linked.
- The files the service looks up next to its own executable
  (`std::env::current_exe()`, overridable with `PERFR_FRIDA_AGENT` and
  `PERFR_FRIDA_HELPER`):
  - `libperfr_frida_agent.so`
  - `perfr-frida-helper`
- `libperfr_api.so` and `perfr.h`, for manual instrumentation.
- `perfr-gpu-helper`, the optional NVIDIA helper. It is a glibc binary on
  purpose (the static service cannot `dlopen`).
- `licenses/` for the Frida devkits.

macOS, on `macos-15`, via `tools/build/build-service-macos.sh --universal`:

- One universal `perfr` (Apple silicon and Intel), plus
  `libperfr_frida_agent.dylib`, `perfr-frida-helper`, `libperfr_api.dylib`,
  `perfr.h`, and the Frida licenses.
- Ad-hoc signed (`codesign --sign -`), not notarized.
- Records manual scopes and Frida function hooks. No call-stack sampling,
  scheduler, live disassembly, or auto-profiling.
- No GPU helper. That binary is Linux-only.

Windows x86-64, on `windows-2022`, via `.\tools\build\build-service-windows.ps1`:

- `perfr.exe` with the viewer embedded, linked against the static C
  runtime (`.cargo/config.toml`), so it depends on system DLLs only;
  the script checks that with `dumpbin /dependents`.
- `perfr_frida_agent.dll` and `perfr-frida-helper.exe` beside it, for
  function hooks; `perfr_api.dll`, `perfr_api.dll.lib`, `perfr_api.lib` and
  `include\perfr.h` for manual instrumentation; `perfr_api.py`;
  `PerfrTestRust.exe`; the Frida licenses.
- Records manual scopes and Frida hooks without privilege. Call-stack
  sampling, the scheduler and thread states come from the NT Kernel Logger
  (ETW) and need an elevated prompt. No auto-profiling, no kernel hook
  engine, no GPU helper.
- Not code-signed.

Linux arm64 is not in the release. The service compiles for arm64; it is not
run in CI, and the static musl build in this workflow is x86-64 only.

The Linux and macOS archives are gzipped tars, the Windows one a zip; each
has the top-level directory `perfr/`. Beside the archives the release has
`SHA256SUMS` and a `.sha256` file per archive (`sha256sum` format: hash, two
spaces, file name). `tools/install/install.ps1` is the Windows installer and
reads the zip's `.sha256` the way `install.sh` reads the tars'.

## How to cut one

From the commit you want to ship:

```sh
git tag perfr-v0.1.0
git push origin perfr-v0.1.0
```

Pushing the tag starts the workflow. The workflow does not create the tag.
Wait until both build jobs and the publish job are green, then check
`https://github.com/perfr/perfr/releases/tag/perfr-v0.1.0`.

`workflow_dispatch` (Actions → release-rust → Run workflow) builds the same
artifacts and uploads them to the run. Leave **tag** empty: that does not
publish and does not create a tag. Set **tag** to a `perfr-v*` tag that
already exists only when you want that run to create the GitHub Release for
it (`gh release create --verify-tag`).

The Cargo version (`0.1.0` today) is independent of the git tag. The pip
wheels are not: they publish from the same `perfr-v*` tags, and their version
has to match across the files listed in
[`tools/release/README.md`](../tools/release/README.md). Pushing `perfr-v*`
also starts `.github/workflows/python-wheels.yml`, which publishes to PyPI
when the `pypi` environment's Trusted Publishing is configured. Do not push
the tag if you do not want that publish attempt.

## Installer host

The installers are fetched from `https://perfr.dev/install.sh` and
`https://perfr.dev/install.ps1`; `tools/site/build_site.py` copies them to
the site root. [`tools/install/install.sh`](../tools/install/install.sh)
downloads the archives from GitHub's `/releases/latest/download/`. The only
line to change if perfr.dev serves the same file names is `DEFAULT_BASE` at
the top of that script. `PERFR_INSTALL_BASE` overrides it for one run.

Until a stable `perfr-v*` release exists, the script exits with an error that
says so and points at a source build. It does not install a C++ nightly.

## Legacy C++ nightlies

[`.github/workflows/release.yml`](../.github/workflows/release.yml) no longer
has a weekly schedule. A manual run still builds the Qt UI and C++
`PerfrService`, and still uploads a **prerelease** titled as a legacy C++
nightly. The pinned release action has no `make_latest` input; `prerelease:
true` is what keeps GitHub from selecting it as Latest. Existing
`nightly-2026-09-24`, `nightly-2026-10-01`, and `nightly-2026-10-08` releases
are unchanged.
