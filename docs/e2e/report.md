# Orbit e2e report

Run 2026-09-04 17:51 at commit `7495227f3` on `rixbox`.

| Scenario | Result | Time | Note |
|---|---|---|---|
| viewer-idle | pass | 9.6s |  |
| processes | pass | 0.0s |  |
| symbols | pass | 0.4s |  |
| function-search | pass | 0.0s |  |
| capture-scheduling | pass | 20.0s |  |
| sampling-report | pass | 9.1s |  |
| call-trees | pass | 9.1s |  |
| selection-report | pass | 9.1s |  |
| report-tabs | pass | 61.4s |  |
| api-rust | pass | 19.0s | 230007 events, 153 links |
| api-c | pass | 19.0s | 234744 events, 154 links |
| api-cpp | pass | 19.0s | 238158 events, 154 links |
| api-python | pass | 19.0s | 127037 events, 113 links |
| self-instrumentation | pass | 18.0s | 2 segment(s), 234619 events, 145 links (not drawn yet) |
| thread-states | pass | 7.1s | skipped: no scheduling tracepoints (needs CAP_PERFMON) |
| instrumentation | pass | 6.1s | skipped: no hooks armed: uprobes need CAP_PERFMON |
| thread-focus | pass | 20.6s | thread 1123092 of pid 1123092 |
| scope-report | pass | 27.9s | 'render': 903 samples over 301 instances |
| live-tab | pass | 11.1s | 300 rows, histogram for 'solve contacts' |
| flame-tab | pass | 11.4s | ok |
| save-slice-open | pass | 10.7s | 2109 KB bundle, 217 KB slice, 18838 events reopened |
| python-reader | pass | 1.0s | 196499 events; 3 agent rows, 1011 value rows |
| agent-scopes | pass | 18.5s | tracks ['agent'] |
| service-lanes | pass | 9.3s | 1 value lane(s) under pid 1121725 |
| clear | pass | 10.1s | ok |
| wire-and-perf | pass | 32.0s | wire packed, 508 KB/s on the socket |
| website | pass | 21.0s | 258154 events from a 2935 KB stream, 6.3 s to first events |
| hook-from-report | pass | 30.3s | hooked 'b3MulW'; arming skipped: no hooks armed: uprobes need CAP_PERFMON |

## Numbers

- wire: packed
- events: 258154
- ws_bps_during_capture: 520577
- bundle_bytes: 2159705
- slice_bytes: 222795
- stream_bytes: 3006051
- site_first_events_s: 6.3
- service status: {"events_live": 258154, "events_capacity": 8388608, "ring_bytes": 268435456, "produced": 258154, "dropped": 0}

Viewer self-profile (headless Chrome, SwiftShader: slower than a GPU):

| Phase | Total ms | Count | Avg us | Max us |
|---|---|---|---|---|
| Frame | 64.06 | 30 | 2135.3 | 8875.0 |
| TimelinePayload | 12.15 | 59 | 205.8 | 5660.0 |
| ClipLabels | 5.78 | 43 | 134.3 | 230.0 |
| ChooseLod | 4.55 | 59 | 77.0 | 220.0 |
| PrimitiveListing | 3.92 | 15 | 261.7 | 2155.0 |
| Chrome | 3.83 | 30 | 127.8 | 180.0 |
| PoolDispatch | 2.87 | 15 | 191.3 | 1650.0 |
| Rasterize | 2.25 | 16 | 140.3 | 190.0 |
| Tracks | 1.67 | 59 | 28.2 | 60.0 |
| ToRgba8 | 1.2 | 16 | 75.3 | 95.0 |
| Scheduler | 1.08 | 59 | 18.2 | 35.0 |
| PaintHeaders | 0.92 | 59 | 15.5 | 25.0 |
| Net | 0.77 | 30 | 25.8 | 220.0 |
| ListingFlatten | 0.67 | 15 | 44.7 | 395.0 |
| HandleInput | 0.63 | 59 | 10.6 | 505.0 |
| RemapTheme | 0.6 | 16 | 37.5 | 50.0 |
| DrainNet | 0.47 | 30 | 15.7 | 210.0 |
| RasterWalk | 0.29 | 16 | 18.1 | 75.0 |
| ApplyHighlights | 0.27 | 15 | 18.0 | 130.0 |
| ScalePpp | 0.24 | 15 | 15.7 | 115.0 |
| PaintCallback | 0.09 | 59 | 1.4 | 10.0 |
| TickFollow | 0.08 | 30 | 2.5 | 10.0 |
| PlaceExtent | 0.05 | 16 | 3.1 | 5.0 |
| Upload | 0.04 | 28 | 1.3 | 5.0 |
| ListingSort | 0.01 | 15 | 0.7 | 5.0 |

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
