# Hook test report

## workload

| function | outcome | detail |
|---|---|---|
| `tanhf32x` | received | 1362696 calls |
| `__expm1_fma` | received | 1239780 calls |
| `render_frame` | received | 1989 calls |
| `ai_tick` | received | 2012 calls |
| `audio_mix` | received | 2258 calls |
| `__syscall_cancel_arch` | received | 2110 calls |
| `worker` | no-events | armed, but no calls were recorded (not exercised in the window) |
| `main` | no-events | armed, but no calls were recorded (not exercised in the window) |

## crash-demo

| function | outcome | detail |
|---|---|---|
| `crash_in_here` | crash | the target died (SIGSEGV) while only crash_in_here was hooked |
| | | <pre>Program received signal SIGSEGV, Segmentation fault.<br>#0  0x00005555555551d9 in crash_in_here (p=0x0) at tools/hook_test/crash_demo.c:16<br>#1  0x0000555555555266 in main () at tools/hook_test/crash_demo.c:26</pre> |
