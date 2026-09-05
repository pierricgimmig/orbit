//! RAII self-profile scopes. No timers or allocations when disabled.

use std::cell::RefCell;

use orbit_live_event::dev::{query_disables_dev, query_enables_dev, RelScope, VIEWER_PID};

#[cfg(not(target_arch = "wasm32"))]
use orbit_live_event::dev::now_ns as shared_now_ns;

pub struct DevFrame {
    inner: Option<DevFrameInner>,
}

struct DevFrameInner {
    origin_ns: u64,
    /// Worker spans kept / refused this frame, so the UI can tell "no pool"
    /// apart from "the guard ate them".
    absorbed: std::cell::Cell<u32>,
    dropped: std::cell::Cell<u32>,
    scopes: RefCell<Vec<RelScope>>,
    stack: RefCell<Vec<(u32, u32, u64)>>,
}

pub struct DevScope<'a> {
    frame: &'a DevFrame,
    active: bool,
    tid: u32,
    name_id: u32,
    start_rel: u64,
    depth: u8,
}

impl DevFrame {
    pub fn begin(enabled: bool) -> Self {
        if !enabled {
            return Self { inner: None };
        }
        Self {
            inner: Some(DevFrameInner {
                origin_ns: now_ns(),
                absorbed: std::cell::Cell::new(0),
                dropped: std::cell::Cell::new(0),
                scopes: RefCell::new(Vec::with_capacity(16)),
                stack: RefCell::new(Vec::with_capacity(8)),
            }),
        }
    }

    pub fn scope(&self, tid: u32, name_id: u32) -> DevScope<'_> {
        let Some(inner) = self.inner.as_ref() else {
            return DevScope {
                frame: self,
                active: false,
                tid: 0,
                name_id: 0,
                start_rel: 0,
                depth: 0,
            };
        };
        let start_rel = now_ns().saturating_sub(inner.origin_ns);
        let depth = inner
            .stack
            .borrow()
            .iter()
            .filter(|(t, _, _)| *t == tid)
            .count() as u8;
        inner.stack.borrow_mut().push((tid, name_id, start_rel));
        DevScope {
            frame: self,
            active: true,
            tid,
            name_id,
            start_rel,
            depth,
        }
    }

    /// The clock reading this frame's `start_rel_ns` values are relative to,
    /// so a consumer can place scopes on an absolute axis. `None` when the
    /// frame is not instrumented.
    pub fn origin_ns(&self) -> Option<u64> {
        self.inner.as_ref().map(|i| i.origin_ns)
    }

    /// Records a main-thread span from clock readings taken elsewhere, at the
    /// depth currently open on `tid` -- so a phase measured inside a library
    /// call lands as a child of the scope wrapping that call. Ignored when the
    /// readings predate the frame or are inverted.
    pub fn record_span(&self, tid: u32, name_id: u32, t0_ns: u64, t1_ns: u64) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        if t0_ns < inner.origin_ns || t1_ns < t0_ns {
            return;
        }
        let depth = inner
            .stack
            .borrow()
            .iter()
            .filter(|(t, _, _)| *t == tid)
            .count() as u8;
        inner.scopes.borrow_mut().push(RelScope {
            pid: VIEWER_PID,
            tid,
            name_id,
            start_rel_ns: t0_ns - inner.origin_ns,
            duration_ns: t1_ns.saturating_sub(t0_ns).max(1),
            depth,
        });
    }

    /// (worker spans kept, refused) so far this frame.
    pub fn worker_span_counts(&self) -> (u32, u32) {
        match self.inner.as_ref() {
            None => (0, 0),
            Some(i) => (i.absorbed.get(), i.dropped.get()),
        }
    }

    pub fn finish(self) -> Vec<RelScope> {
        match self.inner {
            None => Vec::new(),
            Some(inner) => inner.scopes.into_inner(),
        }
    }

    /// A tid is a lane, and a lane must hold non-overlapping intervals --
    /// `Lane::first_ending_after` binary-searches on that. Spans arrive from
    /// other threads with their own clock readings, so this drops anything that
    /// would break the invariant rather than writing a lane that cannot be
    /// searched. A dropped worker scope is a missing bar; an overlapping one
    /// corrupts every lookup in that lane.
    pub fn absorb_worker_spans(&self, spans: &[orbit_live_render::WorkerSpan]) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let mut ordered: Vec<&orbit_live_render::WorkerSpan> = spans
            .iter()
            // Before the frame even started: a clock that is not comparable
            // with `origin_ns`. Saturating these to 0 is what stacked every
            // pool worker's span on the same spot.
            .filter(|s| s.t0_ns >= inner.origin_ns && s.t1_ns >= s.t0_ns)
            .collect();
        inner
            .dropped
            .set(inner.dropped.get() + (spans.len() - ordered.len()) as u32);
        ordered.sort_by_key(|s| (s.tid, s.t0_ns));
        let mut last_end: Option<(u32, u64)> = None;
        for s in ordered {
            if let Some((tid, end)) = last_end {
                if tid == s.tid && s.t0_ns < end {
                    inner.dropped.set(inner.dropped.get() + 1);
                    continue;
                }
            }
            let dur = s.t1_ns.saturating_sub(s.t0_ns).max(1);
            last_end = Some((s.tid, s.t0_ns.saturating_add(dur)));
            inner.absorbed.set(inner.absorbed.get() + 1);
            inner.scopes.borrow_mut().push(RelScope {
                pid: VIEWER_PID,
                tid: s.tid,
                name_id: s.name_id,
                start_rel_ns: s.t0_ns - inner.origin_ns,
                duration_ns: dur,
                depth: 0,
            });
        }
    }
}

impl Drop for DevScope<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Some(inner) = self.frame.inner.as_ref() else {
            return;
        };
        let dur = now_ns()
            .saturating_sub(inner.origin_ns)
            .saturating_sub(self.start_rel);
        inner.stack.borrow_mut().pop();
        // Keep a 1 ns floor so a clock that does not tick still emits the
        // scope. Skipping duration-0 scopes made d1/d2 lanes appear and
        // vanish between frames and jumped the thread block height.
        inner.scopes.borrow_mut().push(RelScope {
            pid: VIEWER_PID,
            tid: self.tid,
            name_id: self.name_id,
            start_rel_ns: self.start_rel,
            duration_ns: dur.max(1),
            depth: self.depth,
        });
    }
}

#[allow(dead_code)]
pub fn query_dev_from_location() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .map(|s| query_enables_dev(&s))
            .unwrap_or(true)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        query_enables_dev("")
    }
}

/// `?dev=0` / `?self=0` — Record starts demo only.
pub fn query_dev_locked_off_from_location() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .map(|s| query_disables_dev(&s))
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        query_disables_dev("")
    }
}

/// `?report=flat|top_down|bottom_up|modules` — open the report panel on a
/// given tab.
///
/// A deep link is worth having on its own, and it is what makes the
/// screenshot suite deterministic: egui paints to a canvas, so there is no
/// DOM node for a tab pill to click. Automating the UI otherwise means
/// synthesising mouse events at hard-coded coordinates, which breaks the
/// first time a pill moves.
pub fn query_report_tab_from_location() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| query_report_tab(&s))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// `?capture=<url>` -- open a capture stream file (the `stream` export)
/// instead of connecting to a service: the static web page's mode. The
/// URL is relative to the page.
pub fn query_capture_url_from_location() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| query_capture_url(&s))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn query_capture_url(search: &str) -> Option<String> {
    let value = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|kv| kv.strip_prefix("capture="))?;
    if value.is_empty() {
        return None;
    }
    Some(percent_decode(value))
}

/// Enough of percent-decoding for a path: `%2F`, `%3A` and friends.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `?collapse=scheduler` -- start with the machine-wide scheduler track
/// folded, so a process's own lanes are in view without scrolling. On a
/// 32-core box the scheduler alone is taller than a screenshot, and the
/// screenshot suite exists to show the process, not the cores.
pub fn query_collapse_scheduler_from_location() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .map(|s| query_collapses_scheduler(&s))
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

/// Parses `collapse=` out of a location query string; `scheduler` is the
/// only value so far.
#[cfg(any(target_arch = "wasm32", test))]
pub fn query_collapses_scheduler(search: &str) -> bool {
    search
        .trim_start_matches('?')
        .split('&')
        .any(|pair| pair == "collapse=scheduler")
}

/// Parses `report=` out of a location query string.
#[cfg(any(target_arch = "wasm32", test))]
pub fn query_report_tab(search: &str) -> Option<String> {
    for pair in search.trim_start_matches('?').split('&') {
        if let Some(value) = pair.strip_prefix("report=") {
            if !value.is_empty() {
                return Some(value.to_ascii_lowercase());
            }
        }
    }
    None
}

fn now_ns() -> u64 {
    // Same clock as lane-worker spans (`orbit_live_event::dev::now_ns`).
    // WASM: installed hook (`globalThis.performance.now`) so DedicatedWorkers
    // and the UI thread share an origin. Native: Instant.
    #[cfg(target_arch = "wasm32")]
    {
        let hooked = orbit_live_event::dev::now_ns();
        if hooked != 0 {
            return hooked;
        }
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| (p.now() * 1_000_000.0) as u64)
            .unwrap_or(0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        shared_now_ns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::dev::{NAME_FRAME, NAME_NET, TID_NET, TID_UI};

    #[test]
    fn disabled_frame_is_empty() {
        let frame = DevFrame::begin(false);
        {
            let _s = frame.scope(TID_UI, NAME_FRAME);
        }
        assert!(frame.finish().is_empty());
    }

    #[test]
    fn enabled_frame_records_nested_scopes() {
        let frame = DevFrame::begin(true);
        {
            let _outer = frame.scope(TID_UI, NAME_FRAME);
            busy_spin();
            {
                let _inner = frame.scope(TID_NET, NAME_NET);
                busy_spin();
            }
            busy_spin();
        }
        let scopes = frame.finish();
        assert!(scopes.len() >= 2);
        assert_eq!(scopes[0].name_id, NAME_NET);
        assert_eq!(scopes[0].depth, 0);
        assert_eq!(scopes[1].name_id, NAME_FRAME);
        assert_eq!(scopes[1].depth, 0);
        assert_eq!(scopes[1].pid, VIEWER_PID);
        assert!(scopes[1].duration_ns >= scopes[0].duration_ns);
    }

    #[test]
    fn zero_duration_scope_still_emits() {
        let frame = DevFrame::begin(true);
        {
            let _s = frame.scope(TID_UI, NAME_FRAME);
        }
        let scopes = frame.finish();
        assert_eq!(scopes.len(), 1);
        assert!(scopes[0].duration_ns >= 1);
    }

    #[test]
    fn same_thread_children_increment_depth() {
        let frame = DevFrame::begin(true);
        {
            let _outer = frame.scope(TID_UI, NAME_FRAME);
            busy_spin();
            {
                let _inner = frame.scope(TID_UI, NAME_NET);
                busy_spin();
            }
        }
        let scopes = frame.finish();
        let inner = scopes.iter().find(|s| s.name_id == NAME_NET).unwrap();
        let outer = scopes.iter().find(|s| s.name_id == NAME_FRAME).unwrap();
        assert_eq!(inner.depth, 1);
        assert_eq!(outer.depth, 0);
    }

    fn busy_spin() {
        let t0 = now_ns();
        while now_ns().saturating_sub(t0) < 2_000 {}
    }
}

#[cfg(test)]
mod absorb_guard_tests {
    use super::*;
    use orbit_live_render::WorkerSpan;

    fn span(tid: u32, t0: u64, t1: u64) -> WorkerSpan {
        WorkerSpan {
            tid,
            name_id: 30_032,
            t0_ns: t0,
            t1_ns: t1,
        }
    }

    /// Two spans on one tid must never both land in the lane if they overlap.
    #[test]
    fn overlapping_spans_on_one_tid_are_dropped_not_stacked() {
        let f = DevFrame::begin(true);
        let origin = f.inner.as_ref().expect("enabled").origin_ns;
        f.absorb_worker_spans(&[
            span(10, origin + 1_000, origin + 5_000),
            span(10, origin + 3_000, origin + 9_000), // overlaps the first
            span(10, origin + 9_000, origin + 11_000), // clear of both
        ]);
        let out = f.finish();
        assert_eq!(out.len(), 2, "the overlapping span must be dropped");
        let mut ends: Vec<(u64, u64)> = out
            .iter()
            .map(|s| (s.start_rel_ns, s.start_rel_ns + s.duration_ns))
            .collect();
        ends.sort_unstable();
        assert!(
            ends.windows(2).all(|w| w[0].1 <= w[1].0),
            "lane must stay non-overlapping: {ends:?}"
        );
    }

    /// A worker clock that is not comparable with the frame origin used to
    /// saturate to rel 0 and pile every span on one spot.
    #[test]
    fn spans_predating_the_frame_origin_are_dropped() {
        let f = DevFrame::begin(true);
        let origin = f.inner.as_ref().expect("enabled").origin_ns;
        if origin < 10_000 {
            return; // clock too young for this fixture to mean anything
        }
        f.absorb_worker_spans(&[
            span(11, origin - 5_000, origin - 1_000),
            span(11, origin - 4_000, origin - 2_000),
        ]);
        assert!(
            f.finish().is_empty(),
            "pre-origin spans must not be clamped onto rel 0"
        );
    }

    #[test]
    fn a_report_tab_is_read_from_the_query_string() {
        assert_eq!(query_report_tab("?report=bottom_up").as_deref(), Some("bottom_up"));
        assert_eq!(query_report_tab("?dev=0&report=Modules").as_deref(), Some("modules"));
        assert_eq!(query_report_tab("?report="), None);
        assert_eq!(query_report_tab("?dev=0"), None);
        // A different parameter ending in the same letters must not match.
        assert_eq!(query_report_tab("?myreport=flat"), None);
    }

    #[test]
    fn collapse_scheduler_is_read_from_the_query_string() {
        assert!(query_collapses_scheduler("?collapse=scheduler"));
        assert!(query_collapses_scheduler("?report=flat&collapse=scheduler"));
        assert!(!query_collapses_scheduler("?collapse=machine"));
        assert!(!query_collapses_scheduler("?xcollapse=scheduler"));
        assert!(!query_collapses_scheduler(""));
    }
}

#[cfg(test)]
mod capture_url_tests {
    use super::query_capture_url;

    #[test]
    fn the_capture_query_is_read_and_decoded() {
        assert_eq!(query_capture_url("?capture=captures/box3d.orbit.stream"), Some("captures/box3d.orbit.stream".into()));
        assert_eq!(
            query_capture_url("?collapse=scheduler&capture=..%2Fcaptures%2Fa%20b.orbit.stream"),
            Some("../captures/a b.orbit.stream".into())
        );
        assert_eq!(query_capture_url("?capture="), None);
        assert_eq!(query_capture_url("?report=live"), None);
        assert_eq!(query_capture_url("?capture=x%2"), Some("x%2".into()));
    }
}
