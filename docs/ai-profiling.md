# AI profiling in Orbit

Umbrella for AI-related profiling. **Core pillar: learn as much as possible
about an AI workload without changing the user's code or attaching dynamic
instrumentation.** Orbit should be the profiler people reach for to accelerate
AI development, on a laptop or across a cluster.

## Built now (this PR): zero-code workload detection

The cheapest, most reliable zero-code signal is what a process has already
loaded and opened. `ai_detect.rs` reads `/proc/<pid>/maps` and
`/proc/<pid>/fd` — no attach, no pause, no code in the target — and reports:

- **Framework** from mapped libraries: PyTorch (`libtorch`/`libc10`),
  TensorFlow (`libtensorflow`/`_pywrap_tensorflow`), JAX (`jaxlib`/`libxla`),
  ONNX Runtime (`libonnxruntime`).
- **GPU stack** from mapped libraries: CUDA (`libcuda`/`libcudart`/`libcudnn`/
  `libcublas`/`libnvinfer`) or ROCm (`libamdhip64`/`librocm`/`libhsa`).
- **GPU in use** from open device nodes: `/dev/nvidia*` (NVIDIA) or `/dev/kfd`
  (AMD). `/dev/dri/*` is excluded — any GL client opens it, so it is not a
  compute signal.

At capture start the service logs `pid N looks like an AI workload: PyTorch +
NVIDIA GPU (CUDA)`; `orbit-service --detect-ai <pid>` reports it on demand and
lists the framework's suggested hook points.

**Proven end to end** on a real NVIDIA GPU: a test spawns a process that loads
`libcuda` and calls `cuInit` (which opens `/dev/nvidia*`), then detects it
purely from `/proc` — `detects_a_real_gpu_process_without_touching_it`. A
plain process is not flagged (`a_plain_process_is_not_flagged`), and the
classifier rules are unit-tested with fixtures. The GPU test skips cleanly
where there is no NVIDIA device, so CI without a GPU stays green.

## Why this is the foundation

Knowing a process is "PyTorch + CUDA" is what lets everything else stay
zero-code:

- **Auto-hook the training loop.** `suggested_hooks(framework)` names the hot
  native entry points (PyTorch: `at::native::`, `torch::autograd::`, `c10::`,
  `cudaLaunchKernel`; TensorFlow: `tensorflow::OpKernel::Compute`; …). Orbit's
  existing native dynamic instrumentation hooks these by symbol — so a capture
  turns into named framework scopes with the user picking nothing. (Wiring
  the suggestions into the auto-hook path is the next step.)
- **Label the process** in the viewer (an "AI" badge / the framework name),
  so the angle is obvious from the process picker.
- **Data loading vs. compute.** Framework + thread names (`pt_data_worker`,
  DataLoader workers) separate CPU data-loading threads from the compute
  thread, still zero-code.

## GPU kernels and CUDA

Orbit already has the mechanism: the **GPU telemetry helper**
(`telemetry.rs`) is a dynamically linked process that links CUPTI/NVML and
streams pod events to the static service, which lands them as GPU lanes / job
spans. CUDA kernel profiling is CUPTI activity records (kernel launches,
memcpy, occupancy) fed through that helper and aligned on the timeline under
the host scope that launched them. This PR does not add the CUPTI records
(needs the vendor SDK to build and a GPU to verify beyond detection); the
detection above tells Orbit *when* to spin the helper up.

## Cluster-wide

Each machine runs a service; the viewer already aggregates machine rows from
services across the network (one timeline per host, clock-synchronised). The
AI story rides that: a training job across N GPUs is N services, their host +
device timelines merged, each process labelled with its framework/GPU from the
zero-code detection above. The remaining work is result rollups (per-rank
summaries) rather than new capture plumbing.

## Status / checklist

- [x] **Zero-code detection of PyTorch/TensorFlow/JAX/ONNX and GPU use** —
      built and tested end to end (`ai_detect.rs`, `--detect-ai`).
- [x] **GPU usage detection without instrumenting the target** — built and
      proven on a real GPU.
- [~] **Auto-hook the training loop** — the suggested-hook lists exist; wiring
      them into the hook path is next (reuses the native uprobe/Frida path).
- [ ] **CPU data-loading + GPU kernel profiling path** — designed; the GPU
      side is CUPTI records through the telemetry helper.
- [ ] **CUDA profiling integration** — CUPTI activity via the helper; needs
      the SDK + a GPU to build and verify.
- [ ] **Cluster-wide result aggregation** — rides the existing multi-service
      viewer; needs per-rank rollups.

The homepage angle for the no-code promise is delivered by the Python
(`python-ebpf-instrumentation.md`) and Mojo (`mojo-instrumentation.md`)
landing sections.
