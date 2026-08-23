//! RAII self-profile scopes. No timers or allocations when disabled.

use std::cell::RefCell;

use orbit_live_event::dev::{query_enables_dev, RelScope, VIEWER_PID};

pub struct DevFrame {
    inner: Option<DevFrameInner>,
}

struct DevFrameInner {
    origin_ns: u64,
    scopes: RefCell<Vec<RelScope>>,
    stack: RefCell<Vec<(u32, u64, u8)>>,
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
        let depth = inner.stack.borrow().len() as u8;
        inner.stack.borrow_mut().push((name_id, start_rel, depth));
        DevScope {
            frame: self,
            active: true,
            tid,
            name_id,
            start_rel,
            depth,
        }
    }

    pub fn finish(self) -> Vec<RelScope> {
        match self.inner {
            None => Vec::new(),
            Some(inner) => inner.scopes.into_inner(),
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
        if dur == 0 {
            return;
        }
        inner.scopes.borrow_mut().push(RelScope {
            pid: VIEWER_PID,
            tid: self.tid,
            name_id: self.name_id,
            start_rel_ns: self.start_rel,
            duration_ns: dur,
            depth: self.depth,
        });
    }
}

pub fn query_dev_from_location() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .map(|s| query_enables_dev(&s))
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = query_enables_dev;
        false
    }
}

fn now_ns() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| (p.now() * 1_000_000.0) as u64)
            .unwrap_or(0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
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
        assert_eq!(scopes[0].depth, 1);
        assert_eq!(scopes[1].name_id, NAME_FRAME);
        assert_eq!(scopes[1].depth, 0);
        assert_eq!(scopes[1].pid, VIEWER_PID);
        assert!(scopes[1].duration_ns >= scopes[0].duration_ns);
    }

    fn busy_spin() {
        let t0 = now_ns();
        while now_ns().saturating_sub(t0) < 2_000 {}
    }
}
