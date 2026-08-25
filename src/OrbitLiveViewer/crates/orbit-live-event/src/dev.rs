//! Dogfood identity for self-profiling the live viewer / service.
//!
//! Reserved pids (demo already uses `pid = 1`):
//! - [`VIEWER_PID`] `orbit-live-viewer` — WASM/egui `ui` / `render` / `net`
//! - [`SERVICE_PID`] `orbit-live-service` — native HTTP / ring
//!
//! Product choice **A**: self scopes share the active capture ring. When demo
//! or a capture is producing, batches are stamped so they end at
//! `newest_end_ns` (same axis as the target). Otherwise they use wall time
//! from service start. Events are ordinary [`LiveEvent`]s (32 bytes).
//! Enable is **on** by default; `?dev=0` / Dev pill / `/api/self/stop` turn it off.

use serde::{Deserialize, Serialize};

use crate::{color_mode, kind, InternTable, LiveEvent};

pub const VIEWER_PID: u32 = 2;
pub const SERVICE_PID: u32 = 3;
pub const VIEWER_NAME: &str = "orbit-live-viewer";
pub const SERVICE_NAME: &str = "orbit-live-service";

pub const TID_UI: u32 = 1;
pub const TID_RENDER: u32 = 2;
pub const TID_NET: u32 = 3;
pub const TID_SERVER: u32 = 4;

pub const NAME_FRAME: u32 = 30_000;
pub const NAME_NET: u32 = 30_001;
pub const NAME_TRACKS: u32 = 30_002;
pub const NAME_LOD: u32 = 30_003;
pub const NAME_PAYLOAD: u32 = 30_004;
pub const NAME_CHROME: u32 = 30_005;
pub const NAME_PUSH: u32 = 30_007;
pub const NAME_RASTER: u32 = 30_008;
pub const NAME_TIMELINE_API: u32 = 30_009;

/// Relative scope from a client frame. Server remaps onto the capture clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelScope {
    pub pid: u32,
    pub tid: u32,
    pub name_id: u32,
    pub start_rel_ns: u64,
    pub duration_ns: u64,
    pub depth: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RelScopeBatch {
    #[serde(default)]
    pub scopes: Vec<RelScope>,
}

pub fn is_self_pid(pid: u32) -> bool {
    pid == VIEWER_PID || pid == SERVICE_PID
}

pub fn intern_self_names(intern: &mut InternTable) {
    intern.insert_id(TID_UI, "ui");
    intern.insert_id(TID_RENDER, "render");
    intern.insert_id(TID_NET, "net");
    intern.insert_id(TID_SERVER, "server");
    intern.insert_id(NAME_FRAME, "Frame");
    intern.insert_id(NAME_NET, "Net");
    intern.insert_id(NAME_TRACKS, "Tracks");
    intern.insert_id(NAME_LOD, "ChooseLod");
    intern.insert_id(NAME_PAYLOAD, "TimelinePayload");
    intern.insert_id(NAME_CHROME, "Chrome");
    intern.insert_id(NAME_PUSH, "PushEvents");
    intern.insert_id(NAME_RASTER, "Rasterize");
    intern.insert_id(NAME_TIMELINE_API, "TimelineApi");
}

/// Place a relative scope on the capture axis so it ends at `end_ns`.
pub fn stamp_batch(scopes: &[RelScope], end_ns: u64) -> Vec<LiveEvent> {
    let span = scopes
        .iter()
        .map(|s| s.start_rel_ns.saturating_add(s.duration_ns))
        .max()
        .unwrap_or(0);
    let origin = end_ns.saturating_sub(span);
    scopes
        .iter()
        .filter(|s| s.duration_ns > 0)
        .map(|s| LiveEvent {
            start_ns: origin.saturating_add(s.start_rel_ns),
            duration_ns: s.duration_ns,
            tid: s.tid,
            pid: s.pid,
            kind: kind::API_SCOPE,
            depth: s.depth,
            extra: 0,
            _pad: color_mode::AUTO_THREAD,
            name_id: s.name_id,
        })
        .collect()
}

/// Default **on**. `?dev=0` / `false` / `off` force off. `?dev=1` / `?self=1`
/// / bare `?dev` stay on. Other query keys do not disable.
pub fn query_enables_dev(search: &str) -> bool {
    !query_disables_dev(search)
}

/// `?dev=0` / `?self=0` / `false` / `off`.
pub fn query_disables_dev(search: &str) -> bool {
    let s = search.trim().trim_start_matches('?');
    if s.is_empty() {
        return false;
    }
    s.split('&').any(|part| {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let val = kv.next().unwrap_or("1");
        matches!(key, "dev" | "self") && matches!(val, "0" | "false" | "off")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_dev_flags() {
        assert!(query_enables_dev("?dev=1"));
        assert!(query_enables_dev("self=1&foo=bar"));
        assert!(query_enables_dev("?dev"));
        assert!(query_enables_dev(""));
        assert!(query_enables_dev("?other=1"));
        assert!(!query_enables_dev("?dev=0"));
        assert!(!query_enables_dev("?self=false"));
        assert!(!query_enables_dev("dev=off"));
        assert!(query_disables_dev("?dev=0"));
        assert!(!query_disables_dev(""));
    }

    #[test]
    fn stamp_batch_pins_to_live_edge() {
        let scopes = [RelScope {
            pid: VIEWER_PID,
            tid: TID_UI,
            name_id: NAME_FRAME,
            start_rel_ns: 0,
            duration_ns: 1_000,
            depth: 0,
        }];
        let ev = stamp_batch(&scopes, 10_000);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].pid, VIEWER_PID);
        assert_eq!(ev[0].start_ns, 9_000);
        assert_eq!(ev[0].duration_ns, 1_000);
        assert_eq!(ev[0].kind, kind::API_SCOPE);
    }

    #[test]
    fn reserved_pids_are_not_demo() {
        assert_ne!(VIEWER_PID, 1);
        assert_ne!(SERVICE_PID, 1);
        assert_ne!(VIEWER_PID, 10);
        assert_ne!(SERVICE_PID, 11);
        assert_ne!(VIEWER_PID, SERVICE_PID);
        assert!(is_self_pid(VIEWER_PID));
        assert!(is_self_pid(SERVICE_PID));
        assert!(!is_self_pid(1));
    }

    #[test]
    fn intern_self_names_use_tid_and_scope_ids() {
        let mut intern = InternTable::default();
        intern_self_names(&mut intern);
        assert_eq!(intern.get(TID_UI), Some("ui"));
        assert_eq!(intern.get(TID_RENDER), Some("render"));
        assert_eq!(intern.get(NAME_FRAME), Some("Frame"));
        assert_eq!(intern.get(NAME_PAYLOAD), Some("TimelinePayload"));
    }
}
