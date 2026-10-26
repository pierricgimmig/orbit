<!--
Copyright (c) 2026 The Orbit Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Renaming Orbit to perfr

Plan, 2026-10-25. Runs after the repository cleanup (PR #118) has merged,
as a single PR, so the tree is never half-named.

## Size

Outside the blog, the lockfile and binaries, the name appears on 4,504
lines in 372 files. Beyond prose it is:

| Where | Count |
| --- | --- |
| Crate directories and package names (`orbit-*`) | 31 |
| Binaries (`orbit-service`, `orbit-frida-helper`, `orbit-gpu-helper`, `orbit-pod-dump`, `orbit-live-service`, `orbit-scope`, `orbit-scope-dump`) and `OrbitTestRust` | 8 |
| pip packages (`orbit-api`, `orbit-profiler`) and their import names | 2 |
| `ORBIT_*` environment variables and `orbit.h` macros | ~85 distinct |
| Public C functions in `orbit.h` (`orbit_*`) | 31 |
| MCP tool names (`orbit_*`) and the server name | 17 + 1 |
| License headers ("The Orbit Authors") and the LICENSE file | 294 + 1 |
| Paths carrying the name (crates, test programs, `orbit.h`, logos, capture fixtures) | ~40 |
| Rust identifiers (`orbit_*` snake, `Orbit*` Pascal) | 118 + 6 |
| Unreal plugin identifiers (`OrbitProfiler`, `FOrbit*`) | 4 kinds |

Name availability on 2026-10-25: `perfr` is free on crates.io and PyPI
(`perfr-api` too). The GitHub organization `perfr` belongs to the owner,
so the repository becomes `github.com/perfr/perfr`.

## Names

`perfr` is lowercase in prose and in every name that allows it; `Perfr`
appears only where an identifier needs PascalCase.

| Thing | Proposed name |
| --- | --- |
| Service binary and pip package | `perfr` (also the console script); the `orbit-profiler` wheel becomes `perfr` |
| Other binaries | `perfr-frida-helper`, `perfr-gpu-helper`, `perfr-pod-dump`, `perfr-live-service`, `perfr-scope`, `perfr-scope-dump` |
| Crates | `perfr-*`, same suffixes; `use perfr_api::` and so on |
| Public API | `perfr.h`; `perfr_scope()` and the 30 other functions; `PERFR_SCOPE` and the 17 other macros; `libperfr_api.so` / `.dylib`, `perfr_api.dll` / `.lib`; Python import `perfr_api`, pip `perfr-api` |
| API ABI version | bumped: the shared-memory segment name changes, so old and new libraries do not interoperate |
| Environment | `PERFR_*` throughout, no fallback to `ORBIT_*` (nothing is released yet) |
| Runtime paths | `~/.config/perfr/settings.json`; `~/.perfr/logs` (was `~/.orbitprofiler/logs`); `/tmp/perfr-*`; segments `perfr-scopes-<pid>`; pipes `\\.\pipe\perfr-core-*`; the Windows per-user section likewise |
| Capture files | written as `.perfr.stream` and `.perfr.zip`; the readers keep accepting `.orbit.stream` and `.orbit.zip` so existing captures still open |
| Viewer and site | title and wordmark; `window.__perfr_ui`, `__perfr_sel`, `__perfr_self`; `perfr-theme` storage key; `perfr-theme-toggle` ids |
| MCP | server name `perfr`; tools `perfr_*` |
| Test programs | `PerfrTestRust`, `PerfrTestC`, `PerfrTestCpp`, `PerfrTestPython` |
| Unreal plugin | `PerfrProfiler`, `FPerfrProfilerModule`, `FPerfrExternalProfiler`, `FPerfrCpuTraceScope` |
| GitHub | repository `perfr/perfr` (transferred from `pierricgimmig/orbit`; GitHub redirects the old URLs); Pages at `perfr.github.io/perfr` unless a custom domain is set; release tags `perfr-vX.Y.Z`; workflow names and concurrency groups; `repository` in `Cargo.toml`; the installers' download URLs |
| Copyright | "The perfr Authors" in headers and in LICENSE, which keeps a notice that parts derive from Orbit, Copyright 2020 The Orbit Authors, BSD-2-Clause |

The copyright row needs a decision from the owner: the service's logic was
ported from google/orbit, whose BSD-2-Clause license requires its
copyright notice to survive in derived work.

## Steps

1. **Merge #118.** The rename touches most files and every crate
   directory; on top of the unmerged cleanup neither change is reviewable.
2. **One scripted pass**, case-aware (`orbit` to `perfr`, `Orbit` to
   `Perfr`, `ORBIT` to `PERFR`, `OrbitTest` to `PerfrTest`) over every
   tracked text file, then `git mv` for every path with the name in it.
   Excluded from the pass, because they are history written when the
   project was Orbit: `docs/blog/*.html`, `docs/blog/metrics/`, the four
   essays at the top of `docs/`, every `github.com/google/orbit` link, and
   mentions of the `cpp` branch.
3. **A hand pass** on what a script cannot know: the capture-file readers
   (accept both extensions), the settings and log directories, the Unreal
   `.uplugin` and `.Build.cs`, the installers' release URLs and tag
   patterns, the LICENSE text, and prose in the README and the manual
   where "Orbit" was a product name in a sentence.
4. **Frame the history instead of rewriting it**: blog post 27 announcing
   the rename, one line on the blog index, and "formerly Orbit" where the
   README or a design note refers to the past. The 26 posts keep their
   text.
5. **Regenerate what shows the name**: the wasm pack, the 47 e2e
   screenshots (the viewer's chrome shows the wordmark), the site, and the
   media-kit clips. A new logo and favicon are design input; until they
   exist the current files stay under their new names.
6. **Verify**: `cargo test --workspace`; the whole e2e suite (the harness
   is renamed too, so targeted runs are not enough); `build_wheels.sh` for
   Linux; the installers against a test release tagged `perfr-v0.0.0-test`;
   a fresh clone; the macOS and Windows CI jobs.
7. **Transfer the repository to `perfr/perfr` last**, then check the
   release download URLs, the Pages URL and that the Actions secrets and
   the PyPI trusted-publishing binding (which names the repository) moved
   with it.

About one working day of mechanical change plus review.
