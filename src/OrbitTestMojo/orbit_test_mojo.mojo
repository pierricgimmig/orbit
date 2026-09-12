# Orbit's Mojo test program: a host-side work loop that launches a GPU kernel.
#
# Mojo compiles ahead of time to native code, so `step`, `simulate` and friends
# are ordinary ELF symbols that Orbit's native dynamic instrumentation can hook
# with no probes or eBPF; the kernel launched from `step` lands on the GPU
# through the CUDA driver, where Orbit's GPU telemetry sees it.

from std.gpu import block_dim, block_idx, thread_idx
from max.gpu.host import DeviceContext, DeviceBuffer
from std.sys import argv
from std.ffi import external_call
from std.time import perf_counter_ns, sleep

comptime N = 1 << 20
comptime BLOCK = 256


def saxpy_kernel(
    y: Pointer[Float32, MutAnyOrigin],
    x: Pointer[Float32, MutAnyOrigin],
    a: Float32,
    n: Int32,
):
    var i = Int(block_idx.x * block_dim.x + thread_idx.x)
    if i < Int(n):
        y[i] = a * x[i] + y[i]


@no_inline
def simulate(iterations: Int) -> Float64:
    """Pure host work: something for a CPU scope to measure."""
    var acc: Float64 = 0.0
    for i in range(iterations):
        acc += Float64(i % 97) * 0.5
    return acc


@no_inline
def step(ctx: DeviceContext, mut x: DeviceBuffer[DType.float32], mut y: DeviceBuffer[DType.float32], frame: Int) raises:
    """One frame: some CPU work, then a saxpy kernel on the GPU."""
    _ = simulate(1_000_000)
    ctx.enqueue_function[saxpy_kernel](
        y.unsafe_ptr(), x.unsafe_ptr(), Float32(frame % 7) + 1.0, Int32(N),
        grid_dim=(N + BLOCK - 1) // BLOCK, block_dim=BLOCK,
    )
    ctx.synchronize()


@no_inline
def host_step(frame: Int) -> Float64:
    """The CPU-only frame, for machines without a GPU (CI): same host scope,
    no kernel."""
    return simulate(1_000_000) + Float64(frame)


def gpu_loop() raises:
    var ctx = DeviceContext()
    print("device:", ctx.name(), flush=True)
    var x = ctx.enqueue_create_buffer[DType.float32](N)
    var y = ctx.enqueue_create_buffer[DType.float32](N)
    with x.map_to_host() as hx, y.map_to_host() as hy:
        for i in range(N):
            hx[i] = Float32(i)
            hy[i] = 1.0
    var frame = 0
    var t0 = perf_counter_ns()
    while True:
        step(ctx, x, y, frame)
        frame += 1
        if frame % 100 == 0:
            var dt = Float64(perf_counter_ns() - t0) / 1e9
            print("frame", frame, "elapsed", dt, "s", flush=True)
        sleep(0.01)


def host_loop():
    var frame = 0
    while True:
        _ = host_step(frame)
        frame += 1
        if frame % 100 == 0:
            print("frame", frame, "(host only)", flush=True)
        sleep(0.01)


def main() raises:
    # First line: the pid, which is how Orbit's e2e harness finds a target.
    print("pid=", external_call["getpid", Int32](), sep="", flush=True)
    # `--cpu` skips the GPU: same host scope, no kernel, so the program (and
    # Orbit's test of it) runs on a box with no accelerator. A missing device
    # aborts inside the runtime rather than raising, so this is a flag, not a
    # fallback.
    for arg in argv():
        if arg == "--cpu":
            host_loop()
    gpu_loop()
