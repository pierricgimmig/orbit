# Orbit e2e report

Run 2026-09-04 19:55 at commit `552aaff8a` on `rixbox`.

| Scenario | Result | Time | Note |
|---|---|---|---|
| viewer-idle | pass | 9.6s |  |
| processes | pass | 0.0s |  |
| symbols | pass | 0.4s |  |
| function-search | pass | 0.0s |  |
| capture-scheduling | pass | 20.0s |  |
| sampling-report | pass | 9.1s |  |
| call-trees | pass | 9.2s |  |
| selection-report | pass | 9.1s |  |
| report-tabs | pass | 61.2s |  |
| api-rust | pass | 19.2s | 228178 events, 155 links, 1056 pre-start refused |
| api-c | pass | 19.0s | 229121 events, 153 links, 1351 pre-start refused |
| api-cpp | pass | 18.9s | 233739 events, 153 links, 1397 pre-start refused |
| api-python | pass | 18.9s | 123122 events, 110 links, 1054 pre-start refused |
| self-instrumentation | pass | 18.0s | 2 segment(s), 229953 events, 145 links (not drawn yet) |
| thread-states | pass | 7.1s | skipped: no scheduling tracepoints (needs CAP_PERFMON) |
| instrumentation | pass | 6.1s | skipped: no hooks armed: uprobes need CAP_PERFMON |
| thread-focus | pass | 21.4s | thread 1140654 of pid 1140654 |
| scope-report | pass | 25.7s | 'update': 902 samples over 300 instances |
| live-tab | pass | 11.0s | 300 rows, histogram for 'solve contacts' |
| flame-tab | pass | 11.3s | ok |
| save-slice-open | pass | 10.6s | 2106 KB bundle, 215 KB slice, 18346 events reopened |
| python-reader | pass | 0.8s | 197634 events; 3 agent rows, 1004 value rows |
| agent-scopes | pass | 18.3s | tracks ['agent'] |
| service-lanes | pass | 9.5s | 1 value lane(s) under pid 1139179 |
| clear | pass | 10.1s | ok |
| wire-and-perf | pass | 24.0s | wire packed, 457 KB/s on the socket |
| website | pass | 19.4s | 215875 events from a 2549 KB stream, 6.4 s to first events |
| hook-from-report | pass | 29.7s | hooked 'b3MulW'; arming skipped: no hooks armed: uprobes need CAP_PERFMON |

## Numbers

- wire: packed
- events: 215875
- ws_bps_during_capture: 467501
- bundle_bytes: 2157496
- slice_bytes: 220715
- stream_bytes: 2610561
- site_first_events_s: 6.4
- service status: {"events_live": 215875, "events_capacity": 8388608, "ring_bytes": 268435456, "produced": 215875, "dropped": 0}

Viewer self-profile (headless Chrome, SwiftShader: slower than a GPU):

| Phase | Total ms | Count | Avg us | Max us |
|---|---|---|---|---|
| Frame | 5.3 | 1 | 5295.0 | 5295.0 |
| TimelinePayload | 3.31 | 1 | 3315.0 | 3315.0 |
| PrimitiveListing | 1.36 | 1 | 1355.0 | 1355.0 |
| PoolDispatch | 0.97 | 1 | 965.0 | 965.0 |
| Net | 0.4 | 1 | 400.0 | 400.0 |
| DrainNet | 0.39 | 1 | 385.0 | 385.0 |
| ListingFlatten | 0.22 | 1 | 220.0 | 220.0 |
| ClipLabels | 0.18 | 1 | 180.0 | 180.0 |
| ChooseLod | 0.15 | 1 | 150.0 | 150.0 |
| Chrome | 0.13 | 1 | 130.0 | 130.0 |
| ApplyHighlights | 0.11 | 1 | 110.0 | 110.0 |
| ScalePpp | 0.09 | 1 | 95.0 | 95.0 |
| Tracks | 0.05 | 1 | 45.0 | 45.0 |
| Scheduler | 0.03 | 1 | 30.0 | 30.0 |
| PaintHeaders | 0.03 | 1 | 25.0 | 25.0 |
| PaintCallback | 0.01 | 1 | 5.0 | 5.0 |
| ListingSort | 0.0 | 1 | 0.0 | 0.0 |
| TickFollow | 0.0 | 1 | 0.0 | 0.0 |
| HandleInput | 0.0 | 1 | 0.0 | 0.0 |

## Screenshots

- `01-viewer-idle.png` -- ![](../screenshots/01-viewer-idle.png)
- `02-capture-live.png` -- ![](../screenshots/02-capture-live.png)
- `03-report-flat.png` -- ![](../screenshots/03-report-flat.png)
- `04-report-topdown.png` -- ![](../screenshots/04-report-topdown.png)
- `05-report-bottomup.png` -- ![](../screenshots/05-report-bottomup.png)
- `06-report-modules.png` -- ![](../screenshots/06-report-modules.png)
- `07-api-rust.png` -- ![](../screenshots/07-api-rust.png)
- `08-api-c.png` -- ![](../screenshots/08-api-c.png)
- `09-api-cpp.png` -- ![](../screenshots/09-api-cpp.png)
- `10-api-python.png` -- ![](../screenshots/10-api-python.png)
- `11-self-instrumentation.png` -- ![](../screenshots/11-self-instrumentation.png)
- `12-thread-focus.png` -- ![](../screenshots/12-thread-focus.png)
- `13-scope-menu.png` -- ![](../screenshots/13-scope-menu.png)
- `14-scope-report.png` -- ![](../screenshots/14-scope-report.png)
- `15-live-tab.png` -- ![](../screenshots/15-live-tab.png)
- `16-flame-tab.png` -- ![](../screenshots/16-flame-tab.png)
- `17-opened-slice.png` -- ![](../screenshots/17-opened-slice.png)
- `18-agent-track.png` -- ![](../screenshots/18-agent-track.png)
- `19-service-lanes.png` -- ![](../screenshots/19-service-lanes.png)
- `20-cleared.png` -- ![](../screenshots/20-cleared.png)
- `21-self-pane.png` -- ![](../screenshots/21-self-pane.png)
- `22-website.png` -- ![](../screenshots/22-website.png)
- `23-static-viewer.png` -- ![](../screenshots/23-static-viewer.png)
- `24-hook-from-report.png` -- ![](../screenshots/24-hook-from-report.png)
