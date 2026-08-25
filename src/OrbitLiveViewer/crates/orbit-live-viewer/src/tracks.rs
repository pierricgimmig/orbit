//! Machine → process → thread session tree. Leaf lanes sit under a thread.

use std::collections::{HashMap, HashSet};

use orbit_live_event::LaneKey;
use orbit_live_render::{lane_gap, lane_height, sort_thread_leaves, TrackIndex};

pub const MACHINE_H: f32 = 16.0;
pub const PROCESS_H: f32 = 18.0;
pub const THREAD_H: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThreadId {
    pub pid: u32,
    pub tid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowId {
    Machine,
    Process(u32),
    Thread(ThreadId),
    Lane(LaneKey),
}

#[derive(Clone, Copy, Debug)]
pub struct TrackRow {
    pub id: RowId,
    pub y: f32,
    pub height: f32,
}

pub struct TrackStrip {
    pub thread_order: Vec<ThreadId>,
    pub process_order: Vec<u32>,
    pub scale: f32,
    pub machine: String,
    collapsed: HashSet<RowId>,
    hidden: HashSet<ThreadId>,
    y: HashMap<RowId, f32>,
    drag: Option<Drag>,
}

struct Drag {
    thread: ThreadId,
    grab_off: f32,
    pointer_y: f32,
}

impl Default for TrackStrip {
    fn default() -> Self {
        Self {
            thread_order: Vec::new(),
            process_order: Vec::new(),
            scale: 1.0,
            machine: "local".into(),
            collapsed: HashSet::new(),
            hidden: HashSet::new(),
            y: HashMap::new(),
            drag: None,
        }
    }
}

impl TrackStrip {
    pub fn sync(&mut self, index: &TrackIndex, filter_pid: Option<u32>) {
        let mut pids: Vec<u32> = Vec::new();
        let mut threads: Vec<ThreadId> = Vec::new();
        for (key, _) in index.lanes() {
            if let Some(pid) = filter_pid {
                if key.pid != pid
                    && !orbit_live_event::dev::is_self_pid(key.pid)
                    && index.lanes().any(|(k, _)| k.pid == pid)
                {
                    continue;
                }
            }
            if !pids.contains(&key.pid) {
                pids.push(key.pid);
            }
            let th = ThreadId {
                pid: key.pid,
                tid: key.tid,
            };
            if !threads.contains(&th) {
                threads.push(th);
            }
        }
        pids.sort_unstable();
        pids.sort_by_key(|p| {
            if *p == orbit_live_event::dev::VIEWER_PID {
                0u8
            } else if *p == orbit_live_event::dev::SERVICE_PID {
                1
            } else {
                2
            }
        });
        threads.sort_by_key(|t| {
            let rank = if t.pid == orbit_live_event::dev::VIEWER_PID {
                0u8
            } else if t.pid == orbit_live_event::dev::SERVICE_PID {
                1
            } else {
                2
            };
            (rank, t.pid, t.tid)
        });
        self.process_order.retain(|p| pids.contains(p));
        for p in pids {
            if !self.process_order.contains(&p) {
                self.process_order.push(p);
            }
        }
        self.process_order.sort_by_key(|p| {
            if *p == orbit_live_event::dev::VIEWER_PID {
                0u8
            } else if *p == orbit_live_event::dev::SERVICE_PID {
                1
            } else {
                2
            }
        });
        self.thread_order.retain(|t| threads.contains(t));
        for t in threads {
            if !self.thread_order.contains(&t) {
                self.thread_order.push(t);
            }
        }
        let visible: HashSet<RowId> = self
            .skeleton(index, filter_pid)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        self.y.retain(|id, _| visible.contains(id));
        for id in visible {
            self.y.entry(id).or_insert(0.0);
        }
    }

    pub fn toggle(&mut self, id: RowId) {
        if matches!(id, RowId::Lane(_)) {
            return;
        }
        if !self.collapsed.insert(id) {
            self.collapsed.remove(&id);
        }
    }

    pub fn collapsed(&self, id: RowId) -> bool {
        self.collapsed.contains(&id)
    }

    pub fn hidden_count(&self) -> usize {
        self.hidden.len()
    }

    pub fn toggle_hidden(&mut self, t: ThreadId) {
        if !self.hidden.insert(t) {
            self.hidden.remove(&t);
        }
    }

    pub fn show_all_threads(&mut self) {
        self.hidden.clear();
    }

    fn is_shown(&self, t: ThreadId) -> bool {
        !self.hidden.contains(&t)
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn is_dragging_thread(&self, t: ThreadId) -> bool {
        self.drag.as_ref().map(|d| d.thread == t).unwrap_or(false)
    }

    fn shown_order(&self) -> Vec<ThreadId> {
        self.thread_order
            .iter()
            .copied()
            .filter(|t| self.is_shown(*t))
            .collect()
    }

    fn preview_threads(&self) -> Vec<ThreadId> {
        let shown = self.shown_order();
        let Some(d) = &self.drag else {
            return shown;
        };
        if !self.is_shown(d.thread) {
            return shown;
        }
        let dest = self.drop_thread_index(d.pointer_y - d.grab_off + 0.5);
        let mut v: Vec<ThreadId> = shown.into_iter().filter(|t| *t != d.thread).collect();
        let dest = dest.min(v.len());
        v.insert(dest, d.thread);
        v
    }

    fn drop_thread_index(&self, y: f32) -> usize {
        self.drop_thread_index_from_blocks(y)
    }

    fn drop_thread_index_from_blocks(&self, y: f32) -> usize {
        let rest: Vec<ThreadId> = self
            .thread_order
            .iter()
            .copied()
            .filter(|t| self.is_shown(*t))
            .filter(|t| self.drag.as_ref().map(|d| d.thread != *t).unwrap_or(true))
            .collect();
        let mut acc = 0.0;
        for (i, t) in rest.iter().enumerate() {
            let h = self.thread_block_h(*t);
            if y < acc + h * 0.5 {
                return i;
            }
            acc += h;
        }
        rest.len()
    }

    fn thread_block_h(&self, t: ThreadId) -> f32 {
        let scale = self.scale.max(0.01);
        let mut h = THREAD_H * scale;
        if self.collapsed.contains(&RowId::Thread(t)) {
            return h;
        }
        for (id, _) in self.y.iter() {
            if let RowId::Lane(k) = *id {
                if k.pid == t.pid && k.tid == t.tid {
                    h += (lane_height(k) + lane_gap(k)) * scale;
                }
            }
        }
        h
    }

    pub fn tick(&mut self, dt: f32, index: &TrackIndex, filter_pid: Option<u32>) {
        let preview = self.preview_threads();
        let skeleton = self.skeleton_with_threads(index, filter_pid, &preview);
        let mut y = 0.0;
        let k = 1.0 - (-dt / 0.08).exp();
        let mut targets = HashMap::new();
        for (id, h) in &skeleton {
            targets.insert(*id, y);
            y += *h;
        }
        for (id, target) in targets {
            let slot = self.y.entry(id).or_insert(target);
            *slot += (target - *slot) * k;
        }
        if let Some(d) = &self.drag {
            let header = RowId::Thread(d.thread);
            let base = d.pointer_y - d.grab_off;
            if let Some(hy) = self.y.get(&header).copied() {
                let dy = base - hy;
                self.y.insert(header, base);
                for (id, slot) in self.y.iter_mut() {
                    match *id {
                        RowId::Lane(k) if k.pid == d.thread.pid && k.tid == d.thread.tid => {
                            *slot += dy;
                        }
                        _ => {}
                    }
                }
            } else {
                self.y.insert(header, base);
            }
        }
    }

    /// Visible row whose vertical band contains `y` (strip-local, 0 at top).
    pub fn row_at_y(&self, y: f32) -> Option<RowId> {
        self.rows()
            .into_iter()
            .find(|r| y >= r.y && y < r.y + r.height)
            .map(|r| r.id)
    }

    pub fn rows(&self) -> Vec<TrackRow> {
        let mut ids: Vec<RowId> = self.y.keys().copied().collect();
        ids.sort_by(|a, b| {
            self.y
                .get(a)
                .unwrap_or(&0.0)
                .partial_cmp(self.y.get(b).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ids.into_iter()
            .filter_map(|id| {
                let y = *self.y.get(&id)?;
                Some(TrackRow {
                    id,
                    y,
                    height: self.height_of(id),
                })
            })
            .collect()
    }

    /// Leaf lanes only, y matching the rail (headers occupy space above them).
    pub fn layout(&self) -> Vec<(LaneKey, f32)> {
        self.rows()
            .into_iter()
            .filter_map(|r| match r.id {
                RowId::Lane(k) => Some((k, r.y)),
                _ => None,
            })
            .collect()
    }

    pub fn begin_drag(&mut self, thread: ThreadId, lane_y: f32, pointer_y: f32) {
        self.drag = Some(Drag {
            thread,
            grab_off: pointer_y - lane_y,
            pointer_y,
        });
    }

    pub fn update_drag(&mut self, pointer_y: f32) {
        if let Some(d) = &mut self.drag {
            d.pointer_y = pointer_y;
        }
    }

    pub fn end_drag(&mut self) {
        if self.drag.is_some() {
            let preview = self.preview_threads();
            let mut pit = preview.iter();
            self.thread_order = self
                .thread_order
                .iter()
                .map(|t| {
                    if self.is_shown(*t) {
                        pit.next().copied().unwrap_or(*t)
                    } else {
                        *t
                    }
                })
                .collect();
        }
        self.drag = None;
    }

    pub fn total_height(&self) -> f32 {
        self.y
            .keys()
            .map(|id| self.height_of(*id))
            .sum::<f32>()
            .max(
                self.rows()
                    .iter()
                    .map(|r| r.y + r.height)
                    .fold(0.0, f32::max),
            )
    }

    pub fn insert_y(&self) -> Option<f32> {
        let d = self.drag.as_ref()?;
        let dest = self.drop_thread_index_from_blocks(d.pointer_y - d.grab_off + 0.5);
        let rest: Vec<ThreadId> = self
            .thread_order
            .iter()
            .copied()
            .filter(|t| self.is_shown(*t) && *t != d.thread)
            .collect();
        let header = self.y.get(&RowId::Thread(d.thread)).copied().unwrap_or(0.0);
        if dest == 0 {
            return Some(
                rest.first()
                    .and_then(|t| self.y.get(&RowId::Thread(*t)).copied())
                    .unwrap_or(header),
            );
        }
        if dest >= rest.len() {
            if let Some(last) = rest.last() {
                let y = self.y.get(&RowId::Thread(*last)).copied().unwrap_or(0.0);
                return Some(y + self.thread_block_h(*last));
            }
        }
        rest.get(dest)
            .and_then(|t| self.y.get(&RowId::Thread(*t)).copied())
    }

    fn height_of(&self, id: RowId) -> f32 {
        let s = self.scale.max(0.01);
        match id {
            RowId::Machine => MACHINE_H * s,
            RowId::Process(_) => PROCESS_H * s,
            RowId::Thread(_) => THREAD_H * s,
            RowId::Lane(k) => (lane_height(k) + lane_gap(k)) * s,
        }
    }

    fn skeleton(&self, index: &TrackIndex, filter_pid: Option<u32>) -> Vec<(RowId, f32)> {
        self.skeleton_with_threads(index, filter_pid, &self.thread_order)
    }

    fn skeleton_with_threads(
        &self,
        index: &TrackIndex,
        filter_pid: Option<u32>,
        threads: &[ThreadId],
    ) -> Vec<(RowId, f32)> {
        let s = self.scale.max(0.01);
        let mut out = Vec::new();
        if threads.is_empty() && self.process_order.is_empty() {
            return out;
        }
        out.push((RowId::Machine, MACHINE_H * s));
        if self.collapsed.contains(&RowId::Machine) {
            return out;
        }
        let has_filter = filter_pid
            .map(|pid| index.lanes().any(|(k, _)| k.pid == pid))
            .unwrap_or(false);
        for &pid in &self.process_order {
            if has_filter && filter_pid != Some(pid) && !orbit_live_event::dev::is_self_pid(pid) {
                continue;
            }
            if !index.lanes().any(|(k, _)| k.pid == pid) {
                continue;
            }
            if !threads.iter().any(|th| th.pid == pid && self.is_shown(*th)) {
                continue;
            }
            out.push((RowId::Process(pid), PROCESS_H * s));
            if self.collapsed.contains(&RowId::Process(pid)) {
                continue;
            }
            for &th in threads {
                if th.pid != pid || !self.is_shown(th) {
                    continue;
                }
                out.push((RowId::Thread(th), THREAD_H * s));
                if self.collapsed.contains(&RowId::Thread(th)) {
                    continue;
                }
                let mut leaves: Vec<LaneKey> = index
                    .lanes()
                    .map(|(k, _)| k)
                    .filter(|k| k.pid == th.pid && k.tid == th.tid)
                    .collect();
                sort_thread_leaves(&mut leaves);
                for k in leaves {
                    out.push((RowId::Lane(k), (lane_height(k) + lane_gap(k)) * s));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{kind, LiveEvent};

    fn scope(pid: u32, tid: u32, name: u32) -> LiveEvent {
        LiveEvent {
            start_ns: 0,
            duration_ns: 10,
            tid,
            pid,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: name,
        }
    }

    #[test]
    fn same_tid_different_pid_are_separate_threads() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 7, 1));
        idx.insert(scope(4, 7, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        assert_eq!(strip.process_order, vec![1, 4]);
        assert_eq!(strip.thread_order.len(), 2);
        strip.tick(1.0, &idx, None);
        let lanes = strip.layout();
        assert_eq!(lanes.len(), 2);
        assert_ne!(lanes[0].0.pid, lanes[1].0.pid);
    }

    #[test]
    fn collapse_thread_hides_leaf_lanes() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert_eq!(strip.layout().len(), 1);
        let th = strip.thread_order[0];
        strip.toggle(RowId::Thread(th));
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert!(strip.layout().is_empty());
        assert!(strip
            .rows()
            .iter()
            .any(|r| matches!(r.id, RowId::Thread(_))));
    }

    #[test]
    fn capture_filter_keeps_self_pids() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        idx.insert(scope(orbit_live_event::dev::VIEWER_PID, 1, 30_000));
        idx.insert(scope(9, 3, 3));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, Some(1));
        assert!(strip.process_order.contains(&1));
        assert_eq!(
            strip.process_order[0],
            orbit_live_event::dev::VIEWER_PID,
            "self-profile processes stay at the top of the rail"
        );
        assert!(strip
            .process_order
            .contains(&orbit_live_event::dev::VIEWER_PID));
        assert!(!strip.process_order.contains(&9));
    }

    #[test]
    fn drag_end_reorders_threads() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let first = strip.thread_order[0];
        let second = strip.thread_order[1];
        let y0 = strip.y.get(&RowId::Thread(first)).copied().unwrap_or(0.0);
        strip.begin_drag(first, y0, y0);
        strip.update_drag(y0 + 80.0);
        strip.end_drag();
        assert_eq!(strip.thread_order[0], second);
        assert_eq!(strip.thread_order[1], first);
    }

    #[test]
    fn hidden_thread_is_omitted_from_layout() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let first = strip.thread_order[0];
        assert_eq!(strip.layout().len(), 2);
        strip.toggle_hidden(first);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert_eq!(strip.layout().len(), 1);
        assert!(!strip.rows().iter().any(|r| r.id == RowId::Thread(first)));
        assert_eq!(strip.hidden_count(), 1);
        strip.show_all_threads();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert_eq!(strip.layout().len(), 2);
    }

    #[test]
    fn row_at_y_hits_one_visible_row() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let rows = strip.rows();
        assert!(rows.len() >= 3);
        let mid = rows[1];
        assert_eq!(
            strip.row_at_y(mid.y + mid.height * 0.5),
            Some(mid.id)
        );
        assert_eq!(strip.row_at_y(-4.0), None);
    }

    #[test]
    fn drag_skips_hidden_threads() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        idx.insert(scope(1, 3, 3));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let mid = strip.thread_order[1];
        strip.toggle_hidden(mid);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let first = strip.thread_order[0];
        let last = strip.thread_order[2];
        let y0 = strip.y.get(&RowId::Thread(first)).copied().unwrap_or(0.0);
        strip.begin_drag(first, y0, y0);
        strip.update_drag(y0 + 80.0);
        strip.end_drag();
        assert_eq!(strip.thread_order[1], mid);
        assert_eq!(strip.thread_order[0], last);
        assert_eq!(strip.thread_order[2], first);
    }
}
