//! `POST /api/events`: a batch of timeline records produced outside the
//! service -- a work queue, a build system, an agent orchestrator -- filed
//! under processes and threads the caller names itself.
//!
//! `POST /api/scope` covers the one-scope-at-a-time case on a single
//! synthetic process. This endpoint is for a source that already knows its
//! structure: which "process" (a task, a job, a pipeline) each record
//! belongs to, which "thread" inside it (an agent, a worker, a stage), and
//! when each span started and ended. Records land in the same ring as
//! captured events and reach every connected viewer at once, so a running
//! capture and the external activity share one timeline.
//!
//! The caller may stamp records on either clock. `monotonic_ns` is the
//! capture clock (CLOCK_MONOTONIC), the default; `unix_ns` is wall-clock
//! nanoseconds since the epoch, converted here with the offset between the
//! two clocks at the time of the request, so a remote producer never needs
//! to know when this machine booted.

use orbit_live_event::{color_mode, kind, LiveEvent};
use serde::{Deserialize, Serialize};

use crate::LiveService;

/// Which clock the timestamps in an [`EventsBody`] are on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    /// CLOCK_MONOTONIC nanoseconds, the capture clock. The default.
    #[default]
    MonotonicNs,
    /// Nanoseconds since the UNIX epoch.
    UnixNs,
}

/// The request body. Every list is optional; an empty body is accepted and
/// does nothing.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct EventsBody {
    #[serde(default)]
    pub clock: Clock,
    #[serde(default)]
    pub processes: Vec<ProcessName>,
    #[serde(default)]
    pub threads: Vec<ThreadName>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(default)]
    pub instants: Vec<Instant>,
    #[serde(default)]
    pub values: Vec<Value>,
}

/// Names a process row. Sent once; repeated names simply overwrite.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProcessName {
    pub pid: u32,
    pub name: String,
}

/// Names a thread row inside a process.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ThreadName {
    pub pid: u32,
    pub tid: u32,
    pub name: String,
}

/// Where a span is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanTrack {
    /// On the thread's own track, stacked by `depth`. The default.
    #[default]
    Scope,
    /// On the thread's Async track, for work that overlaps itself.
    Async,
}

/// A finished interval. The caller pairs starts and ends itself; a span
/// still running is sent later, or as adjacent segments.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Span {
    pub pid: u32,
    pub tid: u32,
    pub name: String,
    pub start_ns: u64,
    pub duration_ns: u64,
    #[serde(default)]
    pub depth: u8,
    #[serde(default)]
    pub track: SpanTrack,
}

/// A zero-length mark on a thread track.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Instant {
    pub pid: u32,
    pub tid: u32,
    pub name: String,
    pub timestamp_ns: u64,
    #[serde(default)]
    pub depth: u8,
}

/// One sample on a value lane of that name.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Value {
    pub pid: u32,
    pub tid: u32,
    pub name: String,
    pub timestamp_ns: u64,
    pub value: f64,
}

/// What the endpoint answers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct EventsSummary {
    /// Events pushed to the ring (spans, instants and values together).
    pub accepted: u64,
    /// Events refused because they start before the running capture did.
    pub dropped_before_start: u64,
    /// Process and thread names applied.
    pub named: u64,
    /// The capture clock when the request was handled, so a caller on the
    /// other clock can check its conversion.
    pub monotonic_now_ns: u64,
    /// When the current capture began on the capture clock; 0 when none.
    pub capture_start_ns: u64,
}

/// Both clocks read together, so a conversion offset is consistent within
/// one request.
#[derive(Clone, Copy, Debug)]
pub struct ClockNow {
    pub monotonic_ns: u64,
    pub unix_ns: u64,
}

impl ClockNow {
    /// Reads both clocks now.
    pub fn read() -> Self {
        Self {
            monotonic_ns: now_monotonic_ns(),
            unix_ns: now_unix_ns(),
        }
    }

    /// A timestamp on `clock`, expressed on the capture clock. A wall-clock
    /// time before this machine booted has no place on the capture clock
    /// and becomes 0.
    pub fn to_monotonic(self, clock: Clock, timestamp_ns: u64) -> u64 {
        match clock {
            Clock::MonotonicNs => timestamp_ns,
            Clock::UnixNs => {
                let offset = self.unix_ns.saturating_sub(self.monotonic_ns);
                timestamp_ns.saturating_sub(offset)
            }
        }
    }
}

/// Applies a batch to the service: names first, then events, all on the
/// capture clock. Names are interned once per distinct string per request.
pub fn ingest(svc: &LiveService, body: EventsBody, now: ClockNow) -> EventsSummary {
    let mut named = 0u64;
    for process in &body.processes {
        svc.set_process_name(process.pid, &process.name);
        named += 1;
    }
    for thread in &body.threads {
        svc.set_thread_name(thread.pid, thread.tid, &thread.name);
        named += 1;
    }

    let mut ids = NameCache::default();

    let mut events = Vec::with_capacity(body.spans.len() + body.instants.len() + body.values.len());
    for span in &body.spans {
        let event_kind = match span.track {
            SpanTrack::Scope => kind::API_SCOPE,
            SpanTrack::Async => kind::API_TRACK,
        };
        events.push(LiveEvent {
            start_ns: now.to_monotonic(body.clock, span.start_ns),
            duration_ns: span.duration_ns,
            tid: span.tid,
            pid: span.pid,
            kind: event_kind,
            depth: span.depth,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            name_id: ids.id(svc, &span.name),
        });
    }
    for instant in &body.instants {
        events.push(LiveEvent {
            start_ns: now.to_monotonic(body.clock, instant.timestamp_ns),
            duration_ns: 0,
            tid: instant.tid,
            pid: instant.pid,
            kind: kind::API_SCOPE,
            depth: instant.depth,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            name_id: ids.id(svc, &instant.name),
        });
    }
    for value in &body.values {
        events.push(LiveEvent::from_value(
            now.to_monotonic(body.clock, value.timestamp_ns),
            value.pid,
            value.tid,
            ids.id(svc, &value.name),
            value.value as f32,
        ));
    }

    let dropped_before = svc.dropped_before_start();
    svc.push_events(&events);
    let dropped = svc.dropped_before_start() - dropped_before;
    EventsSummary {
        accepted: events.len() as u64 - dropped,
        dropped_before_start: dropped,
        named,
        monotonic_now_ns: now.monotonic_ns,
        capture_start_ns: svc.capture_start_ns(),
    }
}

/// Interns each distinct name once per request. The table would return the
/// same id for a repeat anyway; this saves the broadcast of the string.
#[derive(Default)]
struct NameCache<'b> {
    ids: std::collections::HashMap<&'b str, u32>,
}

impl<'b> NameCache<'b> {
    fn id(&mut self, svc: &LiveService, text: &'b str) -> u32 {
        if let Some(id) = self.ids.get(text) {
            return *id;
        }
        let id = svc.intern_string(text);
        self.ids.insert(text, id);
        id
    }
}

/// The capture clock (CLOCK_MONOTONIC), as perf timestamps use.
#[cfg(unix)]
pub fn now_monotonic_ns() -> u64 {
    let mut timespec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime into a local.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec);
    }
    timespec.tv_sec as u64 * 1_000_000_000 + timespec.tv_nsec as u64
}

/// Without a capture clock, time since this process first asked; enough for
/// the standalone server, which has no perf events to line up with.
#[cfg(not(unix))]
pub fn now_monotonic_ns() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as u64
}

/// Wall-clock nanoseconds since the UNIX epoch.
pub fn now_unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> std::sync::Arc<LiveService> {
        LiveService::new(crate::ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ring_buffer_bytes: 1024 * 32,
            spill_path: None,
            wire: crate::WireFormat::default(),
        })
        .unwrap()
    }

    fn body() -> EventsBody {
        EventsBody {
            clock: Clock::MonotonicNs,
            processes: vec![ProcessName {
                pid: 7001,
                name: "#1 Benchmark".into(),
            }],
            threads: vec![
                ThreadName {
                    pid: 7001,
                    tid: 7001,
                    name: "task".into(),
                },
                ThreadName {
                    pid: 7001,
                    tid: 8001,
                    name: "agent-a".into(),
                },
            ],
            spans: vec![
                Span {
                    pid: 7001,
                    tid: 8001,
                    name: "claim".into(),
                    start_ns: 100,
                    duration_ns: 50,
                    depth: 0,
                    track: SpanTrack::Scope,
                },
                Span {
                    pid: 7001,
                    tid: 8001,
                    name: "explore".into(),
                    start_ns: 110,
                    duration_ns: 20,
                    depth: 1,
                    track: SpanTrack::Async,
                },
            ],
            instants: vec![Instant {
                pid: 7001,
                tid: 8001,
                name: "note".into(),
                timestamp_ns: 120,
                depth: 0,
            }],
            values: vec![Value {
                pid: 7001,
                tid: 7001,
                name: "progress".into(),
                timestamp_ns: 130,
                value: 40.0,
            }],
        }
    }

    #[test]
    fn a_batch_names_rows_and_lands_in_the_ring_in_order() {
        let svc = service();
        let now = ClockNow {
            monotonic_ns: 1_000,
            unix_ns: 5_000,
        };
        let summary = ingest(&svc, body(), now);
        assert_eq!(summary.accepted, 4);
        assert_eq!(summary.dropped_before_start, 0);
        assert_eq!(summary.named, 3);
        assert_eq!(summary.monotonic_now_ns, 1_000);

        let (_, events) = svc.ring().snapshot();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].kind, kind::API_SCOPE);
        assert_eq!((events[0].pid, events[0].tid), (7001, 8001));
        assert_eq!((events[0].start_ns, events[0].duration_ns), (100, 50));
        assert_eq!(events[1].kind, kind::API_TRACK);
        assert_eq!(events[1].depth, 1);
        assert_eq!(
            events[2].duration_ns, 0,
            "an instant is a zero-length scope"
        );
        assert_eq!(events[3].kind, kind::VALUE);
        assert_eq!(events[3].value_f32(), Some(40.0));

        let intern = svc.intern.lock();
        assert_eq!(intern.get(events[0].name_id), Some("claim"));
        assert_eq!(intern.get(events[1].name_id), Some("explore"));
        assert_eq!(intern.get(events[2].name_id), Some("note"));
        assert_eq!(intern.get(events[3].name_id), Some("progress"));
        drop(intern);

        let names = svc.names.lock();
        assert_eq!(
            names.processes.get(&7001).map(String::as_str),
            Some("#1 Benchmark")
        );
        assert_eq!(
            names.threads.get(&(7001, 8001)).map(String::as_str),
            Some("agent-a")
        );
    }

    #[test]
    fn unix_timestamps_are_moved_onto_the_capture_clock() {
        let now = ClockNow {
            monotonic_ns: 1_000,
            unix_ns: 1_700_000_000_000_000_000,
        };
        // Something that happened 400 ns before the request.
        let wall = now.unix_ns - 400;
        assert_eq!(now.to_monotonic(Clock::UnixNs, wall), 600);
        assert_eq!(now.to_monotonic(Clock::MonotonicNs, wall), wall);
        // Before boot: clamped rather than wrapped.
        assert_eq!(now.to_monotonic(Clock::UnixNs, 5), 0);

        let svc = service();
        let mut b = body();
        b.clock = Clock::UnixNs;
        for span in &mut b.spans {
            span.start_ns += now.unix_ns - 1_000;
        }
        b.instants.clear();
        b.values.clear();
        let summary = ingest(&svc, b, now);
        assert_eq!(summary.accepted, 2);
        let (_, events) = svc.ring().snapshot();
        assert_eq!(events[0].start_ns, 100);
        assert_eq!(events[1].start_ns, 110);
    }

    #[test]
    fn events_before_the_capture_start_are_counted_not_kept() {
        let svc = service();
        svc.mark_capture_started(0, 115);
        let summary = ingest(
            &svc,
            body(),
            ClockNow {
                monotonic_ns: 1_000,
                unix_ns: 5_000,
            },
        );
        // The claim (100) is out; explore (110) is out; note (120) and progress (130) stay.
        assert_eq!(summary.dropped_before_start, 2);
        assert_eq!(summary.accepted, 2);
        assert_eq!(summary.capture_start_ns, 115);
        let (_, events) = svc.ring().snapshot();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn an_empty_body_is_accepted_and_does_nothing() {
        let svc = service();
        let summary = ingest(&svc, EventsBody::default(), ClockNow::read());
        assert_eq!(summary.accepted, 0);
        assert_eq!(summary.named, 0);
        assert!(svc.ring().snapshot().1.is_empty());
    }

    #[test]
    fn the_body_has_defaults_for_everything_but_the_ids_and_times() {
        let body: EventsBody = serde_json::from_str(
            r#"{"spans":[{"pid":1,"tid":2,"name":"x","start_ns":3,"duration_ns":4}]}"#,
        )
        .unwrap();
        assert_eq!(body.clock, Clock::MonotonicNs);
        assert_eq!(body.spans[0].depth, 0);
        assert_eq!(body.spans[0].track, SpanTrack::Scope);
        let body: EventsBody =
            serde_json::from_str(r#"{"clock":"unix_ns","values":[{"pid":1,"tid":1,"name":"v","timestamp_ns":9,"value":1.5}]}"#)
                .unwrap();
        assert_eq!(body.clock, Clock::UnixNs);
        assert_eq!(body.values[0].value, 1.5);
    }
}
