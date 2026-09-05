# Generating the manual

[docs/manual/index.html](manual/index.html) is not written by hand. It is produced by
`//src/OrbitManual:GenerateManual`, which starts a target application, starts OrbitService,
connects Orbit to it, takes a capture and then walks the UI screenshotting every feature it
documents.

```
bazel run //src/OrbitManual:GenerateManual
```

That is the whole thing. It writes `docs/manual/` in the workspace, replacing whatever was there.

## Why generate it

A manual written by hand goes stale the first time a dialog changes, and nothing tells you. This
one cannot: every picture in it was taken from the running program a moment before the page was
written, and the prose that goes with a picture lives in the same function that produced it, in
[`src/OrbitManual/Chapters.cpp`](../src/OrbitManual/Chapters.cpp).

The same property makes it an end-to-end test. Generating the manual exercises symbol loading, a
real capture against a real service, the manual instrumentation API, and every analysis view. A
chapter that comes out empty, or a run that exits non-zero, is a feature that stopped working --
not a stale screenshot.

## What it needs

* **A display.** Widgets are asked to paint themselves rather than being read off the screen, so
  the window does not have to be on top or even fully on screen, but Qt still needs a platform
  plugin with OpenGL. The capture window renders through OpenGL and its framebuffer is read back
  separately.
* **Root, for a complete capture.** OrbitService reads kernel tracepoints under
  `/sys/kernel/debug/tracing`, which is `0700 root`. By default the generator asks for it through
  `pkexec`, which prompts. `--service_launcher=sudo` uses sudo instead, `--service_launcher=none`
  runs the service as the current user, and `--service_launcher=external` attaches to an
  OrbitService that is already running.

Without root the manual still generates, but the capture has no scheduling or thread-state tracks
and the call trees are full of unwind errors, because `sched:*` and `task:*` tracepoints could not
be opened.

## Options

| Flag | Default | What it does |
| --- | --- | --- |
| `--output_directory` | `docs/manual` | Where the pages go. A relative path is resolved against the workspace, so `bazel run` writes into the source tree rather than into the runfiles. |
| `--target_application` | the `OrbitTest` next to the binary | The application to profile. |
| `--target_arguments` | `5 10 1000` | OrbitTest's thread count, recursion depth and sleep in microseconds. |
| `--orbit_service` | the `OrbitService` next to the binary | Which service to run. |
| `--service_launcher` | `pkexec` | `pkexec`, `sudo`, `none` or `external`. |
| `--capture_seconds` | `10` | How long the capture runs. |
| `--window_width` / `--window_height` | `2600` / `1400` | The window is opened at a fixed size so that a regenerated manual's screenshots line up with the previous one's. Orbit's own minimum width is around 1800; below roughly 2400 the analysis tabs squeeze the capture window down to a few hundred pixels and its screenshots stop being readable. |

The generator runs under its own `QSettings` application name, so it neither reads nor writes the
window geometry, symbol locations and other settings of the Orbit you use day to day.

## Adding a chapter

Chapters are methods on `Recorder` in
[`src/OrbitManual/Chapters.cpp`](../src/OrbitManual/Chapters.cpp), called in order by
`Recorder::Record`. A chapter fills in a `Chapter`, drives the UI to the state it wants to show,
calls `AddScreenshot` for each picture, and hands the chapter to the manual. Two rules are worth
knowing:

* Find widgets by object name through `GetWidget`/`GetAction` rather than by walking the widget
  tree. Those two fail loudly, so a renamed widget breaks the build's next run instead of quietly
  dropping a chapter.
* Never re-enter the event loop from inside a Qt callback. `AddScreenshot` deliberately does not
  pump; the callers wait before they call it. Re-entering has been enough to trip assertions deep
  inside Orbit's data structures.

## Which target application

OrbitTest, for now. It is a good subject because it uses the whole manual instrumentation API --
scopes, colours, group ids, async scopes and tracked variables -- so a capture of it has something
in every view. Pointing `--target_application` at something else is supported and untested.
