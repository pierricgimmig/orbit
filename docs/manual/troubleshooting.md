# Troubleshooting

## perf_event_paranoid

The service reads `/proc/sys/kernel/perf_event_paranoid` and prints what this process may do. Root, `CAP_PERFMON` (Linux 5.8+) or `CAP_SYS_ADMIN` bypass the sysctl. Otherwise:

| Level | What an unprivileged process may open |
|---|---|
| −1 | No restrictions |
| 0 | System-wide (per-CPU) events, which the scheduler needs |
| 1 | Per-task events only, no system-wide |
| 2 | Per-task user-space events only. Sampling your own process still works |
| 3 | No `perf_event_open` for unprivileged users (the Debian/Ubuntu patch) |

So sampling the target needs paranoid at 2 or below, and the scheduler (system-wide context switches) needs 0 or below. An unreadable paranoid file is treated as 2: the service tries, and the kernel decides.

The report the service prints offers three ways to enable what was denied. One of them:

```
sudo sysctl -w kernel.perf_event_paranoid=0
```

To keep that across reboots, the same report writes:

```
echo 'kernel.perf_event_paranoid=0' | sudo tee /etc/sysctl.d/99-orbit-perf.conf
```

Or grant the binary capabilities, without changing the sysctl. The report’s command is:

```
sudo setcap cap_perfmon,cap_sys_ptrace+ep /path/to/orbit-service
```

On kernels older than 5.8 that line uses `cap_sys_admin` instead of `cap_perfmon`. File capabilities are ignored on a filesystem mounted `nosuid`. Sampling another user’s process also needs `CAP_SYS_PTRACE` and permission to ptrace it (`kernel.yama.ptrace_scope`).

The installer’s closing note says sampling needs paranoid at −1. That is stricter than the service. Trust the table above.

## Uprobes need CAP_SYS_ADMIN

Kernel uprobes are checked in `perf_uprobe_event_init` before `perf_event_paranoid` is consulted. Lowering paranoid does not arm them, and neither does `CAP_PERFMON` alone: that call returns EACCES.

When no probe arms, the instrumentation line is:

```
no hooks armed: uprobes need CAP_SYS_ADMIN. Run the service with sudo, or: sudo setcap cap_sys_admin,cap_perfmon,cap_dac_read_search+ep <orbit-service>
```

Frida does not use that capability. Its note on the HOOKS row is “requires permission to attach to the target”.

`CAP_SYS_ADMIN` is root in all but name. A passwordless sudo rule on a binary you can overwrite is a root shell for anything running as you. Fine on a personal machine, not on a shared one.

## A kernel without uprobes

If the kernel was built without `CONFIG_UPROBE_EVENTS`, opening a uprobe fails with:

```
this kernel has no uprobe PMU (CONFIG_UPROBE_EVENTS)
```

Switch **HOOKS** to **Frida**, or boot a kernel that has the uprobe PMU. There is no userspace flag that adds the PMU.

## Opening or clearing while a capture is running

Clear, and opening a `.orbit.zip`, are refused until you stop:

- `busy: stop the capture before clearing`
- `busy: stop the capture before opening one`

Opening a Chrome or Perfetto trace is different. The viewer stops the capture itself, then replaces the timeline. If a live capture ends the moment you open a `.json` trace, that is this path, not a crash.

## The timeline stays empty

- No process selected, and **CSW** / **States** off, and nothing in the target calling `orbit.h`: there is nothing to draw. Turn on Sample or CSW, or [instrument the code](index.html#instrument-your-code).
- The process row or the service log says CSW needs a lower paranoid value. See the table above.
- Uprobes selected, Functions hooked, and the amber line says no hooks armed. The setcap command is in the line.
- **API** is off, so manual scopes are dropped even though the program calls them.
- The capture clock starts when Record does. Scopes written before that, or still open across the start, are refused. `/api/status` counts them as `dropped_before_start`.
