# Orbit e2e report

Run 2026-09-04 21:22 at commit `075fdc880` on `rixbox`.

| Scenario | Result | Time | Note |
|---|---|---|---|
| viewer-idle | pass | 9.6s |  |
| processes | pass | 0.0s |  |
| symbols | pass | 0.4s |  |
| function-search | pass | 0.0s |  |
| capture-scheduling | pass | 20.4s |  |
| sampling-report | pass | 9.1s |  |
| call-trees | pass | 9.1s |  |
| selection-report | pass | 9.1s |  |
| report-tabs | pass | 61.4s |  |
| api-rust | pass | 19.1s | 226150 events, 154 links, 1815 pre-start refused |
| api-c | pass | 19.0s | 229711 events, 153 links, 1135 pre-start refused |
| api-cpp | pass | 19.1s | 230354 events, 154 links, 1451 pre-start refused |
| api-python | pass | 19.0s | 118425 events, 113 links, 1601 pre-start refused |
| self-instrumentation | pass | 18.1s | 2 segment(s), 222806 events, 145 links (not drawn yet) |
| thread-states | pass | 7.1s | skipped: no scheduling tracepoints (needs CAP_PERFMON) |
| instrumentation | pass | 6.1s | skipped: no hooks armed: uprobes need CAP_PERFMON |
| thread-focus | FAIL | 25.3s | timed out waiting for a thread selection |
| scope-report | FAIL | 10.9s | no right-click on the thread rows landed on a manual scope |
| live-tab | pass | 10.5s | 300 rows, histogram for 'solve contacts' |
| flame-tab | pass | 11.0s | ok |
| save-slice-open | pass | 10.5s | 2134 KB bundle, 224 KB slice, 18790 events reopened |
| python-reader | pass | 0.9s | 198903 events; 3 agent rows, 1007 value rows |
| agent-scopes | pass | 18.4s | tracks ['agent'] |
| service-lanes | pass | 9.4s | 1 value lane(s) under pid 1167570 |
| clear | pass | 10.2s | ok |
| wire-and-perf | FAIL | 41.4s | timed out waiting for the self-profile readout |
| website | pass | 21.4s | 214311 events from a 2551 KB stream, 6.4 s to first events |
| hook-from-report | pass | 31.8s | hooked 'b3MulW'; arming skipped: no hooks armed: uprobes need CAP_PERFMON |
| report-filter | pass | 23.4s | 15 of 200 rows match 'b3Mul' |

## Numbers

- wire: packed
- bundle_bytes: 2186155
- slice_bytes: 230265
- stream_bytes: 2612519
- site_first_events_s: 6.4

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
- `25-report-filter.png` -- ![](../screenshots/25-report-filter.png)
