# Hook test harness

Rock-solid-hooking testbench: it instruments **one function at a time** and
checks the event arrives, so a hook that silently fires nothing — or crashes
the target — is caught function by function, across a list of processes you
choose.

This is the groundwork for auto-instrumenting a big app (say an Unreal game)
from its sampling report: before Orbit picks functions for a user, we need to
know hooking each one is safe and actually produces events.

## Run

```sh
python3 tools/hook_test/hook_harness.py --sudo
python3 tools/hook_test/hook_harness.py --sudo --only workload
python3 tools/hook_test/hook_harness.py --sudo --config my_targets.toml
```

`--sudo` runs the service through the passwordless wrapper
(`tools/sudo/install.sh`), because uprobes need `CAP_SYS_ADMIN`; without it the
run reports the arming refusal and stops. The service binary is
`rust/crates/orbit-service/target/release/orbit-service` (build it first).

Output: a per-function scorecard on the console and in
`docs/hook-test/report.md` (+ `.json`):

- **received** — armed, and N of its calls were recorded. The hook works.
- **no-events** — armed, but nothing arrived in the window (the function was
  not called then; e.g. a one-shot `main`).
- **crash** — the target died while only that function was hooked, so it is
  the culprit. The exit signal is recorded, and with `gdb` the backtrace too.

## Specify processes

Edit `tools/hook_test/targets.toml` — one `[[target]]` block each:

```toml
[[target]]
name   = "workload"                 # label for the report
out    = "/tmp/orbit-hook-workload" # what `build` produces; rebuilt only if missing
build  = "cc -O2 -g -o /tmp/orbit-hook-workload tools/hook_test/workload.c -lpthread -lm"
argv   = ["/tmp/orbit-hook-workload"]
select = "report"                   # report | search:<query> | list:a,b,c
top    = 8                          # how many functions (report/search)
seconds = 3                         # capture seconds per function
wait_go = false                     # process waits for a stdin line before working
gdb     = false                     # run under gdb for a crash backtrace
```

`select`:

- `report` — a short sampling capture, then the hottest functions with a
  hookable id. This is the "hook what the profile says matters" path.
- `search:<query>` — functions whose name matches (e.g. `search:Tick`).
- `list:a,b,c` — exactly these names.

### A released binary (e.g. a game)

```toml
[[target]]
name   = "game"
argv   = ["/opt/MyGame/Binaries/Linux/MyGame"]
select = "search:Tick"
top    = 25
seconds = 5
gdb    = true
```

The two demos shipped here — `workload.c` (named hot functions) and
`crash_demo.c` (crashes on demand) — need no setup and prove both paths: a
hook that fires, and a hook that crashes (caught with its gdb backtrace).

## Notes

- A crash relaunches the target and moves to the next function, so one bad
  hook does not end the run.
- `gdb` targets are single-shot per function (gdb's `run` ends at the crash);
  the demo prints `pid=<n>` so the harness can hook it — a released binary run
  under gdb should do the same, or omit `gdb` and rely on the exit signal.
