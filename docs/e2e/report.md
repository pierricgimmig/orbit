# Orbit e2e report

Run 2026-09-04 14:50 at commit `70ff8b51b` on `rixbox`.

| Scenario | Result | Time | Note |
|---|---|---|---|
| viewer-idle | pass | 9.6s |  |
| processes | pass | 0.0s |  |
| symbols | pass | 0.4s |  |
| function-search | pass | 0.0s |  |
| capture-scheduling | pass | 19.7s |  |
| sampling-report | pass | 9.1s |  |
| call-trees | pass | 9.1s |  |
| selection-report | pass | 9.1s |  |
| report-tabs | pass | 61.1s |  |
| api-rust | pass | 19.0s | 230060 events, 154 links |
| api-c | pass | 19.0s | 229874 events, 153 links |
| api-cpp | pass | 19.0s | 233892 events, 153 links |
| api-python | pass | 19.0s | 123435 events, 113 links |
| self-instrumentation | pass | 18.1s | 2 segment(s), 227698 events, 146 links (not drawn yet) |
| thread-states | pass | 7.1s | skipped: no scheduling tracepoints (needs CAP_PERFMON) |
| instrumentation | pass | 6.1s | skipped: no hooks armed: uprobes need CAP_PERFMON |
| thread-focus | pass | 21.0s | thread 1092478 of pid 1092478 |
| scope-report | pass | 29.5s | 'update': 898 samples over 299 instances |
| live-tab | pass | 11.4s | 300 rows, histogram for 'solve contacts' |
| flame-tab | pass | 11.2s | ok |
| save-slice-open | pass | 10.6s | 2102 KB bundle, 223 KB slice, 18358 events reopened |
| python-reader | pass | 0.9s | 194349 events; 3 agent rows, 1002 value rows |
| agent-scopes | pass | 18.9s | tracks ['agent'] |
| service-lanes | pass | 9.4s | 1 value lane(s) under pid 1091465 |
| clear | pass | 10.0s | ok |
| wire-and-perf | pass | 33.3s | wire packed, 452 KB/s on the socket |

## Numbers

- wire: packed
- events: 197767
- ws_bps_during_capture: 463065
- bundle_bytes: 2153445
- slice_bytes: 228993
- service status: {"events_live": 197767, "events_capacity": 8388608, "ring_bytes": 268435456, "produced": 197767, "dropped": 0}

Viewer self-profile (headless Chrome, SwiftShader: slower than a GPU):

| Phase | Total ms | Count | Avg us | Max us |
|---|---|---|---|---|
| Frame | 51.04 | 30 | 1701.3 | 3860.0 |
| TimelinePayload | 6.51 | 59 | 110.3 | 2545.0 |
| ClipLabels | 5.7 | 33 | 172.9 | 225.0 |
| Chrome | 4.43 | 30 | 147.5 | 750.0 |
| Rasterize | 3.6 | 26 | 138.7 | 180.0 |
| ChooseLod | 3.52 | 59 | 59.7 | 145.0 |
| ToRgba8 | 1.95 | 26 | 75.0 | 90.0 |
| Tracks | 1.71 | 59 | 29.1 | 50.0 |
| Scheduler | 1.1 | 59 | 18.7 | 40.0 |
| RemapTheme | 0.97 | 26 | 37.3 | 55.0 |
| PaintHeaders | 0.93 | 59 | 15.8 | 25.0 |
| PrimitiveListing | 0.79 | 4 | 197.5 | 735.0 |
| Net | 0.63 | 30 | 21.0 | 50.0 |
| RasterWalk | 0.39 | 26 | 15.2 | 60.0 |
| DrainNet | 0.33 | 30 | 10.8 | 35.0 |
| PoolDispatch | 0.32 | 4 | 80.0 | 300.0 |
| ListingFlatten | 0.28 | 4 | 71.2 | 270.0 |
| HandleInput | 0.11 | 59 | 1.9 | 10.0 |
| PaintCallback | 0.1 | 59 | 1.6 | 5.0 |
| TickFollow | 0.1 | 30 | 3.2 | 10.0 |
| ApplyHighlights | 0.09 | 4 | 22.5 | 90.0 |
| ScalePpp | 0.09 | 4 | 22.5 | 85.0 |
| PlaceExtent | 0.09 | 26 | 3.3 | 10.0 |
| Upload | 0.05 | 29 | 1.6 | 5.0 |
| ListingSort | 0.01 | 4 | 1.3 | 5.0 |

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
