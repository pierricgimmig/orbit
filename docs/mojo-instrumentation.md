# Mojo: host and GPU in one timeline

The same "one timeline across the boundary" angle as
[Python](python-ebpf-instrumentation.md), but Mojo makes it **simpler**, not
harder — and the GPU side is a natural fit.

## Why Mojo is the easy case

Python needed CPython's USDT probes because it is *interpreted*: the functions
Orbit wants to see are bytecode inside the interpreter, invisible from outside
without a probe mechanism (and, for in-kernel filtering, eBPF).

**Mojo compiles ahead of time** (through MLIR/LLVM) to native code. A Mojo
function is an ordinary native symbol in the binary. So Orbit does not need a
new probe mechanism at all — its **existing native dynamic instrumentation**
(kernel uprobes, or the Frida engine) hooks a Mojo function exactly the way it
hooks a C, C++ or Rust function: pick it, entry and exit become a scope on the
calling thread, no rebuild of the target. The sampling profiler already
unwinds and symbolizes native frames, so Mojo frames appear in the flame graph
and trees too.

That means most of this feature is *already built* — it is the native path
Orbit ships. What is Mojo-specific is small and is the open work below.

## GPU kernels

Mojo has first-class GPU programming (the Modular / MAX stack): you write the
kernel in Mojo and launch it on the device. The launch goes through the GPU
driver (CUDA/HIP), which is exactly what Orbit's **GPU telemetry helper**
already watches: a dynamically linked helper links CUPTI/NVML and streams pod
events to the static service (`telemetry.rs`), which lands them as GPU lanes /
job spans on the timeline. So a Mojo GPU kernel rides the path that already
exists, aligned under the host function that launched it.

Whether it is *easier* than tracing PyTorch's dispatch: plausibly yes — a Mojo
kernel is compiled with a name you control, and MAX exposes profiling hooks —
but that needs a real Mojo/GPU setup to confirm (see caveats).

## What is Mojo-specific (the open work)

- [ ] **Symbol → source mapping.** Mojo mangles names its own way. Orbit
      demangles Rust (legacy `_ZN..E` and v0) and leaves C names alone; Mojo
      needs its own demangler (or DWARF, if `mojo build` emits it) so the
      hooked symbol reads as the Mojo function the user wrote, not a mangled
      string. Needs a Mojo binary to pin the scheme.
- [ ] **Function discovery / selection.** List a Mojo binary's functions
      (from the symbol table + demangler) so the user can pick from a sampling
      report or search — the same "hook from the report" flow the native side
      has. This is the analog of the Python `--python-probes` discovery, but
      over the symbol table rather than USDT notes.
- [ ] **GPU kernel attribution.** Confirm CUPTI names Mojo kernels usefully,
      and align a kernel span under the host `launch` scope (a link, or by
      thread + time). Investigate whether MAX exposes kernel markers that beat
      generic CUPTI names.
- [ ] **Tagging.** Tag host scopes `language = mojo` and GPU spans as kernels,
      so the viewer shows the source plainly, mirroring the Python plan's
      `language`/`origin` tags.
- [ ] **End-to-end test.** A Mojo program with a host function that launches a
      GPU kernel; verify both appear in one capture, host scope and kernel
      span aligned.

## Fallbacks

- No symbols / stripped Mojo binary: sampling still works (frame-pointer or
  DWARF unwinding); the frames are just unnamed until a demangler/DWARF lands.
- No GPU or no CUPTI: the host side is unaffected; the GPU lane is simply
  empty, as it is for any capture without the helper.

## Status

**Built now (this PR):**
- The **landing-page** section featuring the angle (`tools/site/index.html`):
  "Mojo, host and GPU — one timeline", with a two-lane diagram (Mojo host
  function → GPU kernel).
- This design and the TODO entry.

**Already in the product (reused, not Mojo-specific):**
- Native dynamic instrumentation (uprobes / Frida) that hooks any native
  symbol — which a Mojo function is.
- Sampling + native symbolization; the GPU telemetry helper.

**Not built, and not verifiable here:** everything in "open work" above. This
box has **no Mojo toolchain and no GPU**, so the Mojo mangling scheme, whether
`mojo build` emits DWARF, and how CUPTI names Mojo kernels are all
unconfirmed — the claims in this doc about Mojo internals should be treated as
the plan to validate, not measured facts. The first concrete step is to build
a small Mojo program on a machine with the toolchain, read its symbol table
and any DWARF, and pin the demangling.

## Notes

- Complements the AI umbrella (PyTorch / TensorFlow / CUDA) and the Python
  angle rather than replacing them.
- Because Mojo rides the native path, the highest-leverage Mojo-specific work
  is the demangler + source mapping; the "instrumentation" itself is done.
