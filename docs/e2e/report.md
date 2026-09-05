# Orbit e2e report

Run 2026-09-04 17:04 at commit `fd32f9bac` on `rixbox`.

| Scenario | Result | Time | Note |
|---|---|---|---|
| viewer-idle | pass | 9.6s |  |
| processes | pass | 0.0s |  |
| symbols | pass | 0.4s |  |
| function-search | pass | 0.0s |  |
| capture-scheduling | pass | 19.9s |  |
| sampling-report | pass | 9.1s |  |
| call-trees | pass | 9.1s |  |
| selection-report | pass | 9.1s |  |
| report-tabs | pass | 61.1s |  |
| api-rust | pass | 19.0s | 226477 events, 154 links |
| api-c | pass | 18.9s | 229021 events, 153 links |
| api-cpp | pass | 19.0s | 234513 events, 154 links |
| api-python | pass | 18.9s | 122901 events, 112 links |
| self-instrumentation | pass | 18.2s | 2 segment(s), 232178 events, 146 links (not drawn yet) |
| thread-states | pass | 7.1s | skipped: no scheduling tracepoints (needs CAP_PERFMON) |
| instrumentation | pass | 6.1s | skipped: no hooks armed: uprobes need CAP_PERFMON |
| thread-focus | pass | 21.1s | thread 1114397 of pid 1114397 |
| scope-report | pass | 27.4s | 'render': 901 samples over 300 instances |
| live-tab | pass | 10.9s | 300 rows, histogram for 'solve contacts' |
| flame-tab | pass | 11.3s | ok |
| save-slice-open | pass | 10.8s | 2109 KB bundle, 216 KB slice, 18604 events reopened |
| python-reader | pass | 0.9s | 198468 events; 3 agent rows, 1009 value rows |
| agent-scopes | pass | 18.7s | tracks ['agent'] |
| service-lanes | pass | 9.4s | 1 value lane(s) under pid 1112966 |
| clear | pass | 10.0s | ok |
| wire-and-perf | pass | 29.5s | wire packed, 467 KB/s on the socket |
| website | pass | 21.0s | 202326 events from a 2402 KB stream, 6.5 s to first events |

## Numbers

- wire: packed
- events: 202326
- ws_bps_during_capture: 477829
- bundle_bytes: 2159820
- slice_bytes: 222085
- stream_bytes: 2460013
- site_first_events_s: 6.5
- service status: {"events_live": 202326, "events_capacity": 8388608, "ring_bytes": 268435456, "produced": 202326, "dropped": 0}

Viewer self-profile (headless Chrome, SwiftShader: slower than a GPU):

| Phase | Total ms | Count | Avg us | Max us |
|---|---|---|---|---|
| Frame | 50.47 | 30 | 1682.2 | 4855.0 |
| TimelinePayload | 7.12 | 59 | 120.6 | 3150.0 |
| ClipLabels | 5.54 | 37 | 149.9 | 210.0 |
| Chrome | 3.73 | 30 | 124.3 | 155.0 |
| ChooseLod | 3.39 | 59 | 57.4 | 135.0 |
| Rasterize | 3.14 | 22 | 142.7 | 190.0 |
| Tracks | 1.69 | 59 | 28.6 | 60.0 |
| ToRgba8 | 1.68 | 22 | 76.4 | 90.0 |
| PrimitiveListing | 1.42 | 8 | 177.5 | 1270.0 |
| Scheduler | 1.09 | 59 | 18.5 | 35.0 |
| PaintHeaders | 0.98 | 59 | 16.6 | 30.0 |
| PoolDispatch | 0.97 | 8 | 121.9 | 910.0 |
| RemapTheme | 0.88 | 22 | 39.8 | 55.0 |
| Net | 0.47 | 30 | 15.7 | 40.0 |
| RasterWalk | 0.37 | 22 | 16.6 | 70.0 |
| ListingFlatten | 0.24 | 8 | 30.0 | 210.0 |
| DrainNet | 0.23 | 30 | 7.7 | 30.0 |
| HandleInput | 0.16 | 59 | 2.6 | 25.0 |
| ScalePpp | 0.11 | 8 | 13.1 | 95.0 |
| ApplyHighlights | 0.11 | 8 | 13.1 | 90.0 |
| PlaceExtent | 0.09 | 22 | 3.9 | 10.0 |
| PaintCallback | 0.08 | 59 | 1.3 | 5.0 |
| TickFollow | 0.06 | 30 | 2.0 | 5.0 |
| Upload | 0.02 | 29 | 0.7 | 5.0 |
| ListingSort | 0.01 | 8 | 0.6 | 5.0 |

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
