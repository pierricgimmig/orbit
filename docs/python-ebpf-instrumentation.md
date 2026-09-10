# Python + native in one timeline (eBPF/USDT dynamic instrumentation)

Orbit's differentiator for the AI/ML audience: **dynamic instrumentation on
both sides of the Python boundary, in one capture.** Python functions, the C
extension they call into (NumPy, PyTorch), and the CUDA kernel that launches —
aligned on a single timeline. No other profiler does both in one view.

## The angle

- CPython built `--with-dtrace` / `--enable-systemtap` embeds **USDT probes**.
  The two that matter for a scope per Python function are
  `python:function__entry` and `python:function__return`.
- Those probes catch pure-Python bytecode functions **from outside the
  process** — no imports, no decorators, no restart.
- They do **not** fire for C extensions. In AI workloads the interesting work
  lives there, and on the GPU.
- Orbit's native instrumentation already catches the C-extension and CUDA
  side, so a single capture can show the full stack: Python wrapper → C
  extension → GPU kernel.

## Reality check (measured 2026-09-10)

The premise "present on Ubuntu's default Python" is optimistic. On this box,
`/usr/bin/python3.14` **is** built `--with-dtrace`, but its `.note.stapsdt`
carries only `gc__start`, `gc__done`, `import__find__load__{start,done}` and
`audit` — **not** `function__entry`/`function__return` (the per-function
probes add a semaphore check to every bytecode call, so some builds omit
them). So detect-and-degrade is not an edge case; it is the common path, and
the feature must handle it as a first-class outcome, not a failure.

`orbit-service --python-probes <pid | path>` reports exactly this today (see
"Built now").

## Design

Two ways to attach to the USDT probe location:

1. **Generated eBPF** (the plan). Build a small eBPF program from the set of
   functions to instrument (up to ~32), attach it to `function__entry` /
   `function__return`, and **filter on the function name in-kernel** so
   non-matching calls are dropped before they reach userspace — no per-call
   context switch, no string work on the hot path. The program records a
   timestamped entry/exit into a perf/ring buffer; **no direct callback into
   userspace.** The Orbit service polls the buffer and turns each record into a
   normal Orbit API event. Compile + verify for a program this small is a few
   ms — negligible at capture start; the program is loaded on start and
   unloaded on stop. Keep a small library of **pre-verified templates** (one
   per common pattern) rather than generating fresh code per capture, to avoid
   verifier edge cases.

2. **Perf-uprobe on the probe address** (a cheaper first cut that reuses
   Orbit's existing, tested uprobe path). The `.note.stapsdt` note gives the
   probe's instruction address and its argument descriptor; Orbit can open a
   uprobe there with the same `perf_event_open` machinery it already uses for
   native functions. The tradeoff: no in-kernel name filter, so every
   bytecode call surfaces and Orbit filters/samples in userspace — fine for a
   handful of hot functions, noisy for "instrument everything". This is the
   incremental bridge to (1).

Either way:

- **Tagging.** Every such event carries the existing `source =
  dynamic_instrumentation` **and** `language = python` / `origin =
  python_function`, so the viewer shows plainly that the scope came from a
  real Python function in a Python process — distinct from native dynamic
  instrumentation.
- **Merge.** Python-tagged events join native instrumentation and CUDA events
  on one timeline: Python frames, C-extension frames, and GPU kernels aligned.
- **Selection.** Choose the Python functions to instrument from a sampling
  report, or edit the list in Orbit — the same "hook from the report" flow the
  native side already has.

## Fallbacks and caveats

- **No function probes** (the common case above): degrade to sampled call
  stacks for the Python side; native + CUDA instrumentation are unaffected.
- **Not a dtrace build / no `.note.stapsdt`:** sampling.
- **Windows:** no out-of-process USDT/uprobe story yet (eBPF-for-Windows lacks
  uprobe/USDT support) — sampling.
- **Noise:** the function probes fire for every bytecode call; filter
  (in-kernel with eBPF, or in userspace with uprobes) and/or sample on hot
  paths.

## Status

**Built now (this PR):**

- USDT probe **discovery** (`rust/crates/orbit-service/src/python_usdt.rs`):
  parse `.note.stapsdt`, list a Python image's probes, and decide
  USDT-vs-sampling from whether the function probes are present. Unit-tested,
  and validated against the live `python3.14`.
- CLI: `orbit-service --python-probes <pid | path>` prints the probes and the
  degrade decision.
- The **landing-page** section featuring the angle (`tools/site/index.html`).

**Planned (tracked, needs a `--with-dtrace` Python with the function probes,
privileges, and a GPU to build and verify):**

- [ ] Attach: eBPF templates with in-kernel name filtering, or the
      perf-uprobe bridge, loaded on capture start / unloaded on stop.
- [ ] Ring-buffer poll → Orbit API events, tagged `language = python` /
      `origin = python_function`.
- [ ] Merge Python + native + CUDA on one timeline.
- [ ] Select Python functions from the sampling report / edit in Orbit.
- [ ] End-to-end test: a Python training loop calling a C extension / CUDA
      kernel; verify both sides appear in one capture with correct
      Python-function tagging.

## Landing page

- [x] Dedicated section + diagram: "Python on one side, native + CUDA on the
      other, one timeline", high on the homepage.
- [x] The zero-code story: attach Orbit, see your Python functions *and* the
      kernels they trigger — no imports, no restarts.
- [x] Tied to the AI pillar (single machine and cluster-wide).

## Notes

- This complements the AI umbrella (PyTorch / TensorFlow / CUDA hookups)
  rather than replacing it.
- Related to the manual `orbit-api` Python bindings (the *in-process*,
  code-change path); the USDT/eBPF path here is the *zero-code, out-of-process*
  one.
