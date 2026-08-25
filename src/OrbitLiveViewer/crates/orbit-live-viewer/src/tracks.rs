//! Machine → process → thread session tree. Leaf lanes sit under a thread.

use std::collections::{HashMap, HashSet};

use orbit_live_event::LaneKey;
use orbit_live_render::{lane_gap, lane_height, sort_thread_leaves, TrackIndex};

pub const MACHINE_H: f32 = 16.0;
pub const PROCESS_H: f32 = 18.0;
pub const THREAD_H: f32 = 20.0;

enum SkelItem {
    Row(RowId, f32),
    Hole(f32),
}

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
    cached_rows: Vec<TrackRow>,
    cached_layout: Vec<(LaneKey, f32)>,
    cached_total_h: f32,
    filter_pid: Option<u32>,
    cached_insert_y: Option<f32>,
}

struct Drag {
    thread: ThreadId,
    grab_off: f32,
    pointer_y: f32,
    dest: usize,
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
            cached_rows: Vec::new(),
            cached_layout: Vec::new(),
            cached_total_h: 0.0,
            filter_pid: None,
            cached_insert_y: None,
        }
    }
}

impl TrackStrip {
    pub fn sync(&mut self, index: &TrackIndex, filter_pid: Option<u32>) {
        self.filter_pid = filter_pid;
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
        let multi_demo = pids
            .iter()
            .filter(|p| !orbit_live_event::dev::is_self_pid(**p))
            .count()
            >= 2;
        for p in pids {
            if !self.process_order.contains(&p) {
                self.process_order.push(p);
                if multi_demo && !orbit_live_event::dev::is_self_pid(p) {
                    self.collapsed.insert(RowId::Process(p));
                }
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
        self.apply_layout(index, filter_pid);
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

    pub fn dragging_thread(&self) -> Option<ThreadId> {
        self.drag.as_ref().map(|d| d.thread)
    }

    pub fn row_on_thread(row: RowId, t: ThreadId) -> bool {
        match row {
            RowId::Thread(th) => th == t,
            RowId::Lane(k) => k.pid == t.pid && k.tid == t.tid,
            _ => false,
        }
    }

    fn shown_order(&self) -> Vec<ThreadId> {
        self.thread_order
            .iter()
            .copied()
            .filter(|t| self.is_shown(*t))
            .collect()
    }

    fn rest_threads(&self) -> Vec<ThreadId> {
        self.shown_order()
            .into_iter()
            .filter(|t| self.drag.as_ref().map(|d| d.thread != *t).unwrap_or(true))
            .collect()
    }

    fn process_rest(&self, pid: u32) -> Vec<ThreadId> {
        self.rest_threads()
            .into_iter()
            .filter(|t| t.pid == pid)
            .collect()
    }

    fn process_is_listed(&self, pid: u32) -> bool {
        if let Some(fp) = self.filter_pid {
            if fp != pid && !orbit_live_event::dev::is_self_pid(pid) {
                return false;
            }
        }
        self.thread_order
            .iter()
            .any(|t| t.pid == pid && self.is_shown(*t))
    }

    /// Top of the thread list for `pid` in rail Y space (machine + process
    /// headers + packed rest blocks, no hole).
    fn process_thread_list_y(&self, pid: u32) -> f32 {
        let s = self.scale.max(0.01);
        if self.collapsed.contains(&RowId::Machine) {
            return 0.0;
        }
        let mut y = MACHINE_H * s;
        for &p in &self.process_order {
            if !self.process_is_listed(p) {
                continue;
            }
            y += PROCESS_H * s;
            if self.collapsed.contains(&RowId::Process(p)) {
                continue;
            }
            if p == pid {
                return y;
            }
            for t in self.process_rest(p) {
                y += self.thread_block_h(t);
            }
        }
        y
    }

    fn drop_index_in_process(&self) -> usize {
        let Some(d) = &self.drag else {
            return 0;
        };
        if !self.is_shown(d.thread) {
            return 0;
        }
        let header_top = d.pointer_y - d.grab_off;
        let rest = self.process_rest(d.thread.pid);
        let mut y = self.process_thread_list_y(d.thread.pid);
        for (i, t) in rest.iter().enumerate() {
            let h = self.thread_block_h(*t);
            if header_top < y + h * 0.5 {
                return i;
            }
            y += h;
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

    /// Snap every visible row to its skeleton Y. Collapse must not lerp —
    /// the old 80ms exponential ease made process headers crawl for many frames.
    pub fn tick(&mut self, _dt: f32, index: &TrackIndex, filter_pid: Option<u32>) {
        self.apply_layout(index, filter_pid);
    }

    fn apply_layout(&mut self, index: &TrackIndex, filter_pid: Option<u32>) {
        self.filter_pid = filter_pid;
        let dest = self.drop_index_in_process();
        if let Some(d) = &mut self.drag {
            d.dest = dest;
        }
        let rest = self.rest_threads();
        let skeleton = self.skeleton_with_threads(index, filter_pid, &rest);
        let items = self.skeleton_with_hole(&skeleton, dest);
        let mut y = 0.0;
        let mut next = HashMap::with_capacity(skeleton.len() + 8);
        let mut hole_y = None;
        for item in &items {
            match *item {
                SkelItem::Hole(h) => {
                    hole_y = Some(y);
                    y += h;
                }
                SkelItem::Row(id, h) => {
                    next.insert(id, y);
                    y += h;
                }
            }
        }
        self.cached_insert_y = hole_y;
        if let Some(d) = &self.drag {
            if self.is_shown(d.thread) {
                let base = d.pointer_y - d.grab_off;
                next.insert(RowId::Thread(d.thread), base);
                let s = self.scale.max(0.01);
                let mut ly = base + THREAD_H * s;
                if !self.collapsed.contains(&RowId::Thread(d.thread)) {
                    let mut leaves: Vec<LaneKey> = index
                        .lanes()
                        .map(|(k, _)| k)
                        .filter(|k| k.pid == d.thread.pid && k.tid == d.thread.tid)
                        .collect();
                    sort_thread_leaves(&mut leaves);
                    for k in leaves {
                        next.insert(RowId::Lane(k), ly);
                        ly += (lane_height(k) + lane_gap(k)) * s;
                    }
                }
            }
        }
        self.y = next;
        self.rebuild_rows();
        if let (Some(hy), Some(d)) = (hole_y, self.drag.as_ref()) {
            let hole_h = self.thread_block_h(d.thread);
            self.cached_total_h = self.cached_total_h.max(hy + hole_h);
        }
    }

    fn rebuild_rows(&mut self) {
        let mut ids: Vec<RowId> = self.y.keys().copied().collect();
        ids.sort_by(|a, b| {
            self.y
                .get(a)
                .unwrap_or(&0.0)
                .partial_cmp(self.y.get(b).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.cached_rows.clear();
        self.cached_layout.clear();
        let mut bottom = 0.0_f32;
        let mut height_sum = 0.0_f32;
        for id in ids {
            let Some(&y) = self.y.get(&id) else {
                continue;
            };
            let height = self.height_of(id);
            height_sum += height;
            bottom = bottom.max(y + height);
            if let RowId::Lane(k) = id {
                self.cached_layout.push((k, y));
            }
            self.cached_rows.push(TrackRow { id, y, height });
        }
        self.cached_total_h = height_sum.max(bottom);
    }

    /// Visible row whose vertical band contains `y` (strip-local, 0 at top).
    pub fn row_at_y(&self, y: f32) -> Option<RowId> {
        self.cached_rows
            .iter()
            .find(|r| y >= r.y && y < r.y + r.height)
            .map(|r| r.id)
    }

    pub fn rows(&self) -> &[TrackRow] {
        &self.cached_rows
    }

    /// Leaf lanes only, y matching the rail (headers occupy space above them).
    pub fn layout(&self) -> &[(LaneKey, f32)] {
        &self.cached_layout
    }

    /// Packed rest lanes (no dragged thread). Background raster / instance Ys.
    pub fn rest_layout(&self) -> Vec<(LaneKey, f32)> {
        let Some(d) = &self.drag else {
            return self.cached_layout.clone();
        };
        self.cached_layout
            .iter()
            .copied()
            .filter(|(k, _)| k.pid != d.thread.pid || k.tid != d.thread.tid)
            .collect()
    }

    /// Dragged thread lanes at the floating pointer Y.
    pub fn drag_layout(&self) -> Vec<(LaneKey, f32)> {
        let Some(d) = &self.drag else {
            return Vec::new();
        };
        self.cached_layout
            .iter()
            .copied()
            .filter(|(k, _)| k.pid == d.thread.pid && k.tid == d.thread.tid)
            .collect()
    }

    pub fn begin_drag(&mut self, thread: ThreadId, lane_y: f32, pointer_y: f32) {
        self.drag = Some(Drag {
            thread,
            grab_off: pointer_y - lane_y,
            pointer_y,
            dest: 0,
        });
    }

    pub fn update_drag(&mut self, pointer_y: f32) {
        if let Some(d) = &mut self.drag {
            d.pointer_y = pointer_y;
        }
    }

    pub fn end_drag(&mut self) {
        let Some(d) = self.drag.as_ref() else {
            return;
        };
        if self.is_shown(d.thread) {
            let dest = self.drop_index_in_process();
            let pid = d.thread.pid;
            let thread = d.thread;
            let mut same: Vec<ThreadId> = self
                .thread_order
                .iter()
                .copied()
                .filter(|t| t.pid == pid && self.is_shown(*t) && *t != thread)
                .collect();
            let dest = dest.min(same.len());
            same.insert(dest, thread);
            let mut it = same.iter();
            self.thread_order = self
                .thread_order
                .iter()
                .map(|t| {
                    if t.pid == pid && self.is_shown(*t) {
                        it.next().copied().unwrap_or(*t)
                    } else {
                        *t
                    }
                })
                .collect();
        }
        self.drag = None;
        self.cached_insert_y = None;
    }

    pub fn total_height(&self) -> f32 {
        self.cached_total_h
    }

    pub fn insert_y(&self) -> Option<f32> {
        self.drag.as_ref()?;
        self.cached_insert_y
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

    fn skeleton_with_hole(&self, skeleton: &[(RowId, f32)], dest: usize) -> Vec<SkelItem> {
        let Some(d) = &self.drag else {
            return skeleton
                .iter()
                .map(|(id, h)| SkelItem::Row(*id, *h))
                .collect();
        };
        if !self.is_shown(d.thread) {
            return skeleton
                .iter()
                .map(|(id, h)| SkelItem::Row(*id, *h))
                .collect();
        }
        let hole_h = self.thread_block_h(d.thread);
        let rest_ids = self.process_rest(d.thread.pid);
        let dest = dest.min(rest_ids.len());
        let mut items: Vec<SkelItem> = Vec::with_capacity(skeleton.len() + 1);
        let mut ri = 0usize;
        let mut inserted = false;
        for &(id, h) in skeleton {
            if let RowId::Thread(t) = id {
                if t.pid == d.thread.pid {
                    if ri == dest && !inserted {
                        items.push(SkelItem::Hole(hole_h));
                        inserted = true;
                    }
                    ri += 1;
                }
            }
            items.push(SkelItem::Row(id, h));
        }
        if !inserted {
            if rest_ids.is_empty() {
                if let Some(pos) = items.iter().position(|it| {
                    matches!(it, SkelItem::Row(RowId::Process(p), _) if *p == d.thread.pid)
                }) {
                    items.insert(pos + 1, SkelItem::Hole(hole_h));
                    inserted = true;
                }
            } else if let Some(last) = rest_ids.last() {
                if let Some(pos) = items.iter().rposition(|it| match it {
                    SkelItem::Row(RowId::Thread(t), _) => *t == *last,
                    SkelItem::Row(RowId::Lane(k), _) => k.pid == last.pid && k.tid == last.tid,
                    _ => false,
                }) {
                    items.insert(pos + 1, SkelItem::Hole(hole_h));
                    inserted = true;
                }
            }
        }
        if !inserted {
            items.push(SkelItem::Hole(hole_h));
        }
        items
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
            let keep_drag_proc = self
                .drag
                .as_ref()
                .map(|d| d.thread.pid == pid && self.is_shown(d.thread))
                .unwrap_or(false);
            if !threads.iter().any(|th| th.pid == pid && self.is_shown(*th)) && !keep_drag_proc {
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
        strip.toggle(RowId::Process(1));
        strip.toggle(RowId::Process(4));
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
    fn multi_process_demo_starts_collapsed() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        idx.insert(scope(10, 200, 1));
        idx.insert(scope(11, 300, 1));
        idx.insert(scope(orbit_live_event::dev::VIEWER_PID, 1, 30_000));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert!(strip.collapsed(RowId::Process(1)));
        assert!(strip.collapsed(RowId::Process(10)));
        assert!(strip.collapsed(RowId::Process(11)));
        assert!(!strip.collapsed(RowId::Process(orbit_live_event::dev::VIEWER_PID)));
        assert!(strip
            .rows()
            .iter()
            .any(|r| r.id == RowId::Process(1)));
        assert!(strip
            .rows()
            .iter()
            .any(|r| r.id == RowId::Process(10)));
        assert!(strip
            .rows()
            .iter()
            .any(|r| r.id == RowId::Process(11)));
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
    fn dragging_thread_is_set_while_held() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let first = strip.thread_order[0];
        assert!(strip.dragging_thread().is_none());
        let y0 = strip.y.get(&RowId::Thread(first)).copied().unwrap_or(0.0);
        strip.begin_drag(first, y0, y0);
        assert_eq!(strip.dragging_thread(), Some(first));
        assert!(TrackStrip::row_on_thread(RowId::Thread(first), first));
        strip.end_drag();
        assert!(strip.dragging_thread().is_none());
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
    fn collapse_snaps_ys_in_one_tick() {
        let mut idx = TrackIndex::default();
        for tid in 1..=8u32 {
            idx.insert(scope(1, tid, tid));
        }
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let open_h = strip.total_height();
        assert!(
            !strip.layout().is_empty(),
            "single-process demo starts expanded"
        );
        strip.toggle(RowId::Process(1));
        strip.tick(1.0 / 60.0, &idx, None);
        let snapped = strip.total_height();
        let mut control = TrackStrip::default();
        control.sync(&idx, None);
        control.toggle(RowId::Process(1));
        control.tick(1.0, &idx, None);
        assert!(
            (snapped - control.total_height()).abs() < 0.01,
            "one 16ms tick must land on the final Y, not an 80ms ease ({snapped} vs {})",
            control.total_height()
        );
        assert!(snapped < open_h);
        assert!(strip.layout().is_empty());
        assert_eq!(strip.rows().len(), control.rows().len());
    }

    #[test]
    fn drag_middle_thread_packs_rest_and_moves_hole() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        idx.insert(scope(1, 3, 3));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let t0 = strip.thread_order[0];
        let t1 = strip.thread_order[1];
        let t2 = strip.thread_order[2];
        let y0 = strip.y.get(&RowId::Thread(t0)).copied().unwrap();
        let y1 = strip.y.get(&RowId::Thread(t1)).copied().unwrap();
        let y2 = strip.y.get(&RowId::Thread(t2)).copied().unwrap();
        assert!(y1 > y0 && y2 > y1);
        strip.begin_drag(t1, y1, y1);
        strip.update_drag(y1 + 80.0);
        strip.tick(0.0, &idx, None);
        let y0b = strip.y.get(&RowId::Thread(t0)).copied().unwrap();
        let y2b = strip.y.get(&RowId::Thread(t2)).copied().unwrap();
        assert!(
            (y0b - y0).abs() < 0.01,
            "first rest thread stays put ({y0b} vs {y0})"
        );
        assert!(
            (y2b - y1).abs() < 0.5,
            "origin gap closes so thread 3 packs into thread 2's row ({y2b} vs {y1})"
        );
        let hole = strip.insert_y().expect("insert hole");
        let t2_bottom = y2b + strip.thread_block_h(t2);
        assert!(
            (hole - t2_bottom).abs() < 0.5,
            "insert_y {hole} must match the hole after packed rest ({t2_bottom})"
        );
        let float_y = strip.y.get(&RowId::Thread(t1)).copied().unwrap();
        assert!(
            (float_y - (y1 + 80.0)).abs() < 0.01,
            "dragged thread floats under the pointer"
        );
        assert!(
            strip
                .rest_layout()
                .iter()
                .all(|(k, _)| k.tid != t1.tid),
            "dragged lanes are not reserved in the packed rest skeleton"
        );
        assert!(
            !strip
                .rows()
                .iter()
                .any(|r| r.id == RowId::Thread(t1) && (r.y - y1).abs() < 0.5),
            "dragged header must leave the origin row"
        );
        let origin_lane = y1 + THREAD_H * strip.scale;
        assert!(
            strip
                .rest_layout()
                .iter()
                .all(|(k, y)| k.tid != t1.tid && (*y - origin_lane).abs() > 0.5 || k.tid == t2.tid),
            "no leftover rest instance Y at the vacated origin except the packed neighbor"
        );
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
