//! Machine → process → thread session tree. Leaf lanes sit under a thread.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// A multiplicative hasher for the row and lane keys the layout lives on.
/// SipHash guards against attacker-chosen keys; these are ours, and hashing a
/// few thousand rows several times a frame through it was most of the layout's
/// time on a large capture.
#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        let mut h = self.0;
        h ^= h >> 32;
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^= h >> 29;
        h
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(buf));
        }
    }
    fn write_u8(&mut self, v: u8) {
        self.write_u64(u64::from(v));
    }
    fn write_u16(&mut self, v: u16) {
        self.write_u64(u64::from(v));
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
}

pub type FastState = BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = HashMap<K, V, FastState>;
pub type FastSet<K> = HashSet<K, FastState>;

use orbit_live_event::dev::{is_self_pid, MachineId};
use orbit_live_event::{kind, LaneKey};
use orbit_live_render::{lane_gap, lane_height, sort_thread_leaves, TrackIndex};

pub const MACHINE_H: f32 = 16.0;
pub const PROCESS_H: f32 = 18.0;
pub const SCHEDULER_H: f32 = 18.0;
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
    Scheduler,
    Machine(MachineId),
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
    collapsed: HashSet<RowId>,
    hidden: HashSet<ThreadId>,
    y: FastMap<RowId, f32>,
    drag: Option<Drag>,
    header_drag: Option<HeaderDrag>,
    catalogue: LaneCatalogue,
    cached_rows: Vec<TrackRow>,
    cached_layout: Vec<(LaneKey, f32)>,
    cached_total_h: f32,
    filter_pid: Option<u32>,
    cached_insert_y: Option<f32>,
    layout_gen: u64,
    /// Which process is the target and which is the service: the rail's
    /// default order is target, then instrumented processes by event
    /// count, then the service and the viewer's own rows.
    pub order_hints: OrderHints,
    /// The tier each process was last sorted into (see `process_tier`), so
    /// a header drag stays within its tier.
    process_tier: FastMap<u32, u8>,
    /// Processes already folded for being auto rows (the service, the
    /// viewer), so it happens once; and rows the user toggled by hand,
    /// which the fold never touches. Judged every sync, not on first
    /// sight: the service's pid reaches the app with the first status
    /// message, which can come after its rows do.
    auto_folded: FastSet<u32>,
    user_toggled: FastSet<RowId>,
    /// Chrome `process_sort_index` / `thread_sort_index` (lower first).
    pub process_sort: HashMap<u32, i32>,
    pub thread_sort: HashMap<(u32, u32), i32>,
    /// User order for whole machine trees, overriding MachineId::sort_key.
    pub machine_sort: HashMap<MachineId, i32>,
}

/// Everything the layout needs to know about the index, gathered in one pass
/// and kept until the index's lane set changes.
///
/// Before this, every thread in the layout scanned every lane of the index to
/// find its own -- and did so in the skeleton, again in the Y assignment, and
/// again per block height -- so a frame cost threads x lanes several times
/// over, on a rail that is laid out every frame. The catalogue makes each of
/// those a lookup.
#[derive(Default)]
struct LaneCatalogue {
    /// The `TrackIndex::lane_gen` this was built from; `None` before the first.
    gen: Option<u64>,
    /// Non-CPU lanes per (pid, tid), in draw order.
    leaves: FastMap<(u32, u32), Vec<LaneKey>>,
    /// Threads in order of first appearance in the index.
    threads: Vec<ThreadId>,
    /// Pids that own at least one non-CPU lane.
    pids_with_lanes: FastSet<u32>,
    /// The scheduler core lanes, one per core seen.
    cores: Vec<LaneKey>,
}

impl LaneCatalogue {
    fn build(index: &TrackIndex) -> LaneCatalogue {
        let mut c = LaneCatalogue {
            gen: Some(index.lane_gen()),
            ..LaneCatalogue::default()
        };
        let mut n_cores = 0u16;
        let mut seen_threads: FastSet<(u32, u32)> = FastSet::default();
        // A thread earns a row by saying something: a scope, a sample, a
        // value, a call. Thread-state slices alone -- what every thread of
        // the target gets from the scheduler just for being scheduled --
        // do not, or a capture of a busy process ends as a wall of empty
        // rows for threads that were only ever asleep.
        let mut explicit: FastSet<(u32, u32)> = FastSet::default();
        for (k, lane) in index.lanes() {
            if !is_cpu_lane(k) && k.kind != kind::THREAD_STATE && !is_sampled_frame_lane(k, lane) {
                explicit.insert((k.pid, k.tid));
            }
        }
        for (k, lane) in index.lanes() {
            if is_cpu_lane(k) {
                n_cores = n_cores.max(u16::from(k.extra) + 1);
                continue;
            }
            // A sampled callstack's frames stay in the index -- the report
            // computed here and the sample bar's tooltip read them -- but
            // they are not drawn: a sample is a tick on the sample bar, as
            // in C++ Orbit, not a flame of guessed spans on the thread.
            if is_sampled_frame_lane(k, lane) {
                continue;
            }
            if !explicit.contains(&(k.pid, k.tid)) {
                continue;
            }
            c.pids_with_lanes.insert(k.pid);
            if seen_threads.insert((k.pid, k.tid)) {
                c.threads.push(ThreadId { pid: k.pid, tid: k.tid });
            }
            c.leaves.entry((k.pid, k.tid)).or_default().push(k);
        }
        for leaves in c.leaves.values_mut() {
            sort_thread_leaves(leaves);
        }
        c.cores = (0..n_cores).map(|i| LaneKey::scheduler(i as u8)).collect();
        c
    }

    fn leaves_of(&self, t: ThreadId) -> &[LaneKey] {
        self.leaves.get(&(t.pid, t.tid)).map(Vec::as_slice).unwrap_or(&[])
    }
}

struct Drag {
    thread: ThreadId,
    grab_off: f32,
    pointer_y: f32,
    dest: usize,
}

/// Header rows (processes, machines) reorder by live shuffle rather than the
/// thread drag's float-and-hole affordance: as the pointer crosses a sibling
/// header the order is rewritten in place, so no separate ghost row is drawn.
#[derive(Clone, Copy)]
enum HeaderItem {
    Process(u32),
    Machine(MachineId),
}

struct HeaderDrag {
    item: HeaderItem,
    pointer_y: f32,
}

impl Default for TrackStrip {
    fn default() -> Self {
        Self {
            thread_order: Vec::new(),
            process_order: Vec::new(),
            scale: 1.0,
            collapsed: HashSet::new(),
            hidden: HashSet::new(),
            y: FastMap::default(),
            drag: None,
            header_drag: None,
            catalogue: LaneCatalogue::default(),
            cached_rows: Vec::new(),
            cached_layout: Vec::new(),
            cached_total_h: 0.0,
            filter_pid: None,
            cached_insert_y: None,
            layout_gen: 0,
            order_hints: OrderHints::default(),
            process_tier: FastMap::default(),
            auto_folded: FastSet::default(),
            user_toggled: FastSet::default(),
            process_sort: HashMap::new(),
            thread_sort: HashMap::new(),
            machine_sort: HashMap::new(),
        }
    }
}

/// What the app knows about the processes on the rail that the index does
/// not say: which one the capture targets and which one is the service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrderHints {
    pub target: Option<u32>,
    pub service: Option<u32>,
}

/// The rail's tiers, top to bottom: the target, every other process by
/// what it said, the service and the viewer last. Header drags reorder
/// within a tier; the tiers themselves do not move.
const TIER_TARGET: u8 = 0;
const TIER_INSTRUMENTED: u8 = 1;
const TIER_AUTO: u8 = 2;

impl TrackStrip {
    /// Rebuilds the lane catalogue if the index's lane set changed.
    fn ensure_catalogue(&mut self, index: &TrackIndex) {
        if self.catalogue.gen != Some(index.lane_gen()) {
            self.catalogue = LaneCatalogue::build(index);
        }
    }

    pub fn sync(&mut self, index: &TrackIndex, filter_pid: Option<u32>) {
        self.filter_pid = filter_pid;
        self.ensure_catalogue(index);
        // A filter only narrows the rail when the filtered process actually
        // has lanes; otherwise it would empty the rail before the capture
        // has produced anything.
        let narrow = filter_pid.filter(|p| self.catalogue.pids_with_lanes.contains(p));
        let mut pids: Vec<u32> = Vec::new();
        let mut threads: Vec<ThreadId> = Vec::with_capacity(self.catalogue.threads.len());
        for &th in &self.catalogue.threads {
            if let Some(pid) = narrow {
                if th.pid != pid && !is_self_pid(th.pid) {
                    continue;
                }
            }
            if !pids.contains(&th.pid) {
                pids.push(th.pid);
            }
            threads.push(th);
        }
        pids.sort_unstable();
        // Tier and, within the instrumented tier, how much each process
        // said: events per pid, on a log scale so two processes only swap
        // places when one has twice the other's events, not on every batch.
        let hints = self.order_hints;
        let mut events_per_pid: FastMap<u32, u64> = FastMap::default();
        for (k, lane) in index.lanes() {
            if !is_cpu_lane(k) {
                *events_per_pid.entry(k.pid).or_default() += lane.len() as u64;
            }
        }
        self.process_tier.clear();
        for &p in &pids {
            self.process_tier.insert(p, process_tier(p, hints));
        }
        let rank = |p: u32| -> (u8, i64) {
            let tier = process_tier(p, hints);
            let bucket = if tier == TIER_INSTRUMENTED {
                // Bigger first: the bucket is negated.
                -((events_per_pid.get(&p).copied().unwrap_or(0) + 1).ilog2() as i64)
            } else {
                0
            };
            (tier, bucket)
        };
        pids.sort_by_key(|p| {
            (
                machine_rank(&self.machine_sort, MachineId::from_pid(*p)),
                rank(*p),
                self.process_sort.get(p).copied().unwrap_or(0),
                *p,
            )
        });
        threads.sort_by_key(|t| {
            (
                machine_rank(&self.machine_sort, MachineId::from_pid(t.pid)),
                rank(t.pid),
                self.process_sort.get(&t.pid).copied().unwrap_or(0),
                t.pid,
                self.thread_sort.get(&(t.pid, t.tid)).copied().unwrap_or(0),
                t.tid,
            )
        });
        self.process_order.retain(|p| pids.contains(p));
        // Processes arrive expanded -- collapsing is a deliberate act, and a
        // collapsed track hides the very thing the viewer is for -- except
        // the service's and the viewer's own rows, which are there for when
        // they are wanted and folded until then.
        for p in pids {
            if !self.process_order.contains(&p) {
                self.process_order.push(p);
            }
            if process_tier(p, hints) == TIER_AUTO
                && !self.user_toggled.contains(&RowId::Process(p))
                && self.auto_folded.insert(p)
            {
                self.collapsed.insert(RowId::Process(p));
            }
        }
        self.process_order.sort_by_key(|p| {
            (
                machine_rank(&self.machine_sort, MachineId::from_pid(*p)),
                rank(*p),
                self.process_sort.get(p).copied().unwrap_or(0),
                *p,
            )
        });
        self.thread_order.retain(|t| threads.contains(t));
        for t in threads {
            if !self.thread_order.contains(&t) {
                self.thread_order.push(t);
            }
        }
        // No seeding of `y` here. That used to retain the skeleton's rows and
        // insert the rest at 0 for the lerp animation rows no longer have --
        // and the skeleton lists only the rail lanes, so every frame it
        // evicted the packed flame-graph lanes that apply_layout put straight
        // back. The map never compared equal to itself, layout_gen bumped
        // every frame, and the timeline rebuilt its primitives on every
        // static frame. apply_layout rebuilds the whole map anyway.
        self.apply_layout(index, filter_pid);
    }

    pub fn toggle(&mut self, id: RowId) {
        if matches!(id, RowId::Lane(_)) {
            return;
        }
        if !self.collapsed.insert(id) {
            self.collapsed.remove(&id);
        }
        self.user_toggled.insert(id);
        self.layout_gen = self.layout_gen.wrapping_add(1);
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
        self.layout_gen = self.layout_gen.wrapping_add(1);
    }

    pub fn show_all_threads(&mut self) {
        self.hidden.clear();
        self.layout_gen = self.layout_gen.wrapping_add(1);
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

    /// True while any row -- thread or header -- is being dragged, so the app
    /// keeps repainting and routing pointer moves through the drag handlers.
    pub fn any_dragging(&self) -> bool {
        self.drag.is_some() || self.header_drag.is_some()
    }

    pub fn is_dragging_process(&self, pid: u32) -> bool {
        matches!(
            self.header_drag.as_ref().map(|d| d.item),
            Some(HeaderItem::Process(p)) if p == pid
        )
    }

    pub fn is_dragging_machine(&self, m: MachineId) -> bool {
        matches!(
            self.header_drag.as_ref().map(|d| d.item),
            Some(HeaderItem::Machine(mm)) if mm == m
        )
    }

    pub fn begin_process_drag(&mut self, pid: u32, pointer_y: f32) {
        self.header_drag = Some(HeaderDrag {
            item: HeaderItem::Process(pid),
            pointer_y,
        });
    }

    pub fn begin_machine_drag(&mut self, m: MachineId, pointer_y: f32) {
        self.header_drag = Some(HeaderDrag {
            item: HeaderItem::Machine(m),
            pointer_y,
        });
    }

    /// Reorder live from the pointer's current rail Y. `dest` is the count of
    /// sibling headers whose midpoint sits above the pointer, which is exactly
    /// the insertion slot `reorder_*` wants once the dragged item is removed.
    pub fn update_header_drag(&mut self, pointer_y: f32) {
        let Some(hd) = self.header_drag.as_mut() else {
            return;
        };
        hd.pointer_y = pointer_y;
        let item = hd.item;
        let s = self.scale.max(0.01);
        match item {
            HeaderItem::Process(pid) => {
                let machine = MachineId::from_pid(pid);
                let tier = self.process_tier.get(&pid).copied().unwrap_or(TIER_INSTRUMENTED);
                let dest = self
                    .process_order
                    .iter()
                    .copied()
                    .filter(|p| {
                        *p != pid
                            && MachineId::from_pid(*p) == machine
                            && self.process_tier.get(p).copied().unwrap_or(TIER_INSTRUMENTED) == tier
                    })
                    .filter(|p| {
                        self.y
                            .get(&RowId::Process(*p))
                            .map(|&y| y + PROCESS_H * s * 0.5 < pointer_y)
                            .unwrap_or(false)
                    })
                    .count();
                self.reorder_process(pid, dest);
            }
            HeaderItem::Machine(m) => {
                let dest = self
                    .machines_present()
                    .into_iter()
                    .filter(|mm| *mm != m)
                    .filter(|mm| {
                        self.y
                            .get(&RowId::Machine(*mm))
                            .map(|&y| y + MACHINE_H * s * 0.5 < pointer_y)
                            .unwrap_or(false)
                    })
                    .count();
                self.reorder_machine(m, dest);
            }
        }
    }

    pub fn end_header_drag(&mut self) {
        self.header_drag = None;
    }

    pub fn row_on_thread(row: RowId, t: ThreadId) -> bool {
        match row {
            RowId::Thread(th) => th == t,
            RowId::Lane(k) => !is_cpu_lane(k) && k.pid == t.pid && k.tid == t.tid,
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
            if fp != pid && !is_self_pid(pid) {
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
        let want = MachineId::from_pid(pid);
        let mut y = self.scheduler_block_h();
        for m in self.machines_present() {
            y += MACHINE_H * s;
            if self.collapsed.contains(&RowId::Machine(m)) {
                continue;
            }
            for &p in &self.process_order {
                if MachineId::from_pid(p) != m || !self.process_is_listed(p) {
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
            if m == want {
                return y;
            }
        }
        y
    }

    fn scheduler_block_h(&self) -> f32 {
        let s = self.scale.max(0.01);
        if !self.y.contains_key(&RowId::Scheduler) {
            return 0.0;
        }
        let mut h = SCHEDULER_H * s;
        if self.collapsed.contains(&RowId::Scheduler) {
            return h;
        }
        for (id, _) in self.y.iter() {
            if let RowId::Lane(k) = *id {
                if is_cpu_lane(k) {
                    h += (lane_height(k) + lane_gap(k)) * s;
                }
            }
        }
        h
    }

    fn machine_rank(&self, m: MachineId) -> i64 {
        machine_rank(&self.machine_sort, m)
    }

    fn machines_present(&self) -> Vec<MachineId> {
        let mut out = Vec::new();
        for p in &self.process_order {
            let m = MachineId::from_pid(*p);
            if !out.contains(&m) {
                out.push(m);
            }
        }
        out.sort_by_key(|m| self.machine_rank(*m));
        out
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
        for &k in self.catalogue.leaves_of(t) {
            h += (lane_height(k) + lane_gap(k)) * scale;
        }
        h
    }

    pub fn hidden_in_process(&self, pid: u32) -> usize {
        self.hidden.iter().filter(|t| t.pid == pid).count()
    }

    pub fn show_process_threads(&mut self, pid: u32) {
        self.hidden.retain(|t| t.pid != pid);
        self.layout_gen = self.layout_gen.wrapping_add(1);
    }

    /// Hit test: machine/process/value rows, or the full thread block.
    pub fn hit_at_y(&self, y: f32) -> Option<RowId> {
        if let Some(id) = self.row_at_y(y) {
            return Some(id);
        }
        for t in self.shown_order() {
            if let Some(&ty) = self.y.get(&RowId::Thread(t)) {
                let h = self.thread_block_h(t);
                if y >= ty && y < ty + h {
                    return Some(RowId::Thread(t));
                }
            }
        }
        None
    }

    pub fn thread_band(&self, t: ThreadId) -> Option<(f32, f32)> {
        let y = *self.y.get(&RowId::Thread(t))?;
        Some((y, self.thread_block_h(t)))
    }

    /// Snap every visible row to its skeleton Y. Collapse must not lerp —
    /// the old 80ms exponential ease made process headers crawl for many frames.
    pub fn tick(&mut self, _dt: f32, index: &TrackIndex, filter_pid: Option<u32>) {
        self.apply_layout(index, filter_pid);
    }

    fn apply_layout(&mut self, index: &TrackIndex, filter_pid: Option<u32>) {
        self.filter_pid = filter_pid;
        self.ensure_catalogue(index);
        let dest = self.drop_index_in_process();
        if let Some(d) = &mut self.drag {
            d.dest = dest;
        }
        let rest = self.rest_threads();
        let skeleton = self.skeleton_with_threads(index, filter_pid, &rest);
        let items = self.skeleton_with_hole(&skeleton, dest);
        let mut y = 0.0;
        let mut next: FastMap<RowId, f32> = FastMap::with_capacity_and_hasher(skeleton.len() + 8, FastState::default());
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
                        .filter(|k| {
                            k.pid == d.thread.pid && k.tid == d.thread.tid && !is_cpu_lane(*k)
                        })
                        .collect();
                    sort_thread_leaves(&mut leaves);
                    for k in leaves {
                        next.insert(RowId::Lane(k), ly);
                        ly += (lane_height(k) + lane_gap(k)) * s;
                    }
                }
            }
        }
        self.assign_packed_leaf_ys(index, &mut next);
        if next != self.y {
            self.layout_gen = self.layout_gen.wrapping_add(1);
        }
        self.y = next;
        self.rebuild_rows();
        if let (Some(hy), Some(d)) = (hole_y, self.drag.as_ref()) {
            let hole_h = self.thread_block_h(d.thread);
            self.cached_total_h = self.cached_total_h.max(hy + hole_h);
        }
    }

    fn assign_packed_leaf_ys(&self, _index: &TrackIndex, next: &mut FastMap<RowId, f32>) {
        let s = self.scale.max(0.01);
        for t in self.shown_order() {
            if self.collapsed.contains(&RowId::Thread(t)) {
                continue;
            }
            let Some(&ty) = next.get(&RowId::Thread(t)) else {
                continue;
            };
            let mut ly = ty + THREAD_H * s;
            for &k in self.catalogue.leaves_of(t) {
                if is_rail_lane(k) {
                    continue;
                }
                next.insert(RowId::Lane(k), ly);
                ly += (lane_height(k) + lane_gap(k)) * s;
            }
        }
    }

    fn rebuild_rows(&mut self) {
        // Sort (id, y) pairs outright: the comparator used to look each side
        // up in the map, tens of thousands of hashes per frame on a big rail.
        let mut rows: Vec<(RowId, f32)> = self.y.iter().map(|(id, y)| (*id, *y)).collect();
        rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        self.cached_rows.clear();
        self.cached_layout.clear();
        let mut bottom = 0.0_f32;
        let mut height_sum = 0.0_f32;
        for (id, y) in rows {
            let height = self.height_of(id);
            bottom = bottom.max(y + height);
            if let RowId::Lane(k) = id {
                self.cached_layout.push((k, y));
                if !is_rail_lane(k) && !is_cpu_lane(k) {
                    continue;
                }
            }
            height_sum += height;
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

    pub fn scheduler_core_count_in(index: &TrackIndex) -> usize {
        scheduler_cores(index).len()
    }

    pub fn layout_gen(&self) -> u64 {
        self.layout_gen
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

    /// Move `pid` to slot `dest` among the processes sharing its machine, then
    /// renumber `process_sort` densely so the order survives the next rebuild
    /// (which re-sorts `process_order` by that map). Reorder acts within a
    /// `process_tier`: the target stays first and the service and the
    /// viewer stay last, and the general processes reorder among themselves.
    pub fn reorder_process(&mut self, pid: u32, dest: usize) {
        let machine = MachineId::from_pid(pid);
        let tier_of = |p: u32| self.process_tier.get(&p).copied().unwrap_or(TIER_INSTRUMENTED);
        let mut same: Vec<u32> = self
            .process_order
            .iter()
            .copied()
            .filter(|p| MachineId::from_pid(*p) == machine && tier_of(*p) == tier_of(pid))
            .collect();
        let Some(cur) = same.iter().position(|p| *p == pid) else {
            return;
        };
        same.remove(cur);
        let dest = dest.min(same.len());
        same.insert(dest, pid);
        for (i, p) in same.iter().enumerate() {
            self.process_sort.insert(*p, i as i32);
        }
        let tiers = self.process_tier.clone();
        self.process_order.sort_by_key(|p| {
            (
                machine_rank(&self.machine_sort, MachineId::from_pid(*p)),
                tiers.get(p).copied().unwrap_or(TIER_INSTRUMENTED),
                self.process_sort.get(p).copied().unwrap_or(0),
                *p,
            )
        });
    }

    /// Move `machine` to slot `dest` among the machines on screen, then
    /// renumber `machine_sort` densely so the order sticks across rebuilds.
    pub fn reorder_machine(&mut self, machine: MachineId, dest: usize) {
        let mut order = self.machines_present();
        let Some(cur) = order.iter().position(|m| *m == machine) else {
            return;
        };
        order.remove(cur);
        let dest = dest.min(order.len());
        order.insert(dest, machine);
        for (i, m) in order.iter().enumerate() {
            self.machine_sort.insert(*m, i as i32);
        }
    }

    pub fn total_height(&self) -> f32 {
        self.cached_total_h
    }

    /// Y of the gap the dragged thread will drop into. Nothing paints it any
    /// more -- the layout opening a hole is the drop affordance -- but the
    /// layout invariant is still worth asserting.
    #[allow(dead_code)]
    pub fn insert_y(&self) -> Option<f32> {
        self.drag.as_ref()?;
        self.cached_insert_y
    }

    fn thread_scope_stack_h(&self, t: ThreadId) -> f32 {
        let s = self.scale.max(0.01);
        let mut h = THREAD_H * s;
        if self.collapsed.contains(&RowId::Thread(t)) {
            return h;
        }
        for &k in self.catalogue.leaves_of(t) {
            if !is_rail_lane(k) {
                h += (lane_height(k) + lane_gap(k)) * s;
            }
        }
        h
    }

    fn height_of(&self, id: RowId) -> f32 {
        let s = self.scale.max(0.01);
        match id {
            RowId::Scheduler => SCHEDULER_H * s,
            RowId::Machine(_) => MACHINE_H * s,
            RowId::Process(_) => PROCESS_H * s,
            RowId::Thread(t) => self.thread_scope_stack_h(t),
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

    fn skeleton_with_threads(
        &self,
        index: &TrackIndex,
        filter_pid: Option<u32>,
        threads: &[ThreadId],
    ) -> Vec<(RowId, f32)> {
        let s = self.scale.max(0.01);
        let mut out = Vec::new();
        debug_assert_eq!(self.catalogue.gen, Some(index.lane_gen()), "catalogue is stale");
        let cores = &self.catalogue.cores;
        // The scheduler describes a machine's cores, so it belongs under that
        // machine rather than beside it. It is emitted inside the machine loop
        // below; `scheduler_machine` says which machine owns it.
        if threads.is_empty() && self.process_order.is_empty() {
            // Still show the scheduler when a capture has cores but no
            // process tracks yet -- otherwise a scheduling-only capture looks
            // empty.
            if !cores.is_empty() {
                let m = scheduler_machine();
                out.push((RowId::Machine(m), MACHINE_H * s));
                if !self.collapsed.contains(&RowId::Machine(m)) {
                    push_scheduler_rows(&mut out, cores, self, s);
                }
            }
            return out;
        }
        let has_filter = filter_pid
            .map(|pid| self.catalogue.pids_with_lanes.contains(&pid))
            .unwrap_or(false);
        let mut machines = self.machines_present();
        // A capture may have cores before it has processes on that machine.
        let scheduler_owner = scheduler_machine();
        if !cores.is_empty() && !machines.contains(&scheduler_owner) {
            machines.push(scheduler_owner);
            machines.sort_by_key(|m| m.sort_key());
        }
        for m in machines {
            out.push((RowId::Machine(m), MACHINE_H * s));
            if self.collapsed.contains(&RowId::Machine(m)) {
                continue;
            }
            if m == scheduler_owner && !cores.is_empty() {
                push_scheduler_rows(&mut out, cores, self, s);
            }
            for &pid in &self.process_order {
                if MachineId::from_pid(pid) != m {
                    continue;
                }
                if has_filter && filter_pid != Some(pid) && !is_self_pid(pid) {
                    continue;
                }
                if !self.catalogue.pids_with_lanes.contains(&pid) {
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
                    let leaves = self.catalogue.leaves_of(th);
                    let mut stack = THREAD_H * s;
                    if !self.collapsed.contains(&RowId::Thread(th)) {
                        for k in leaves {
                            if !is_rail_lane(*k) {
                                stack += (lane_height(*k) + lane_gap(*k)) * s;
                            }
                        }
                    }
                    out.push((RowId::Thread(th), stack));
                    if self.collapsed.contains(&RowId::Thread(th)) {
                        continue;
                    }
                    for &k in leaves {
                        if is_rail_lane(k) {
                            out.push((RowId::Lane(k), (lane_height(k) + lane_gap(k)) * s));
                        }
                    }
                }
            }
        }
        out
    }
}

fn is_cpu_lane(k: LaneKey) -> bool {
    k.kind == kind::SCHEDULING_SLICE
}

/// A lane of sampled callstack frames: function-call events the service
/// derives from samples, marked `SAMPLED_FRAME`. One lane holds one kind.
fn is_sampled_frame_lane(k: LaneKey, lane: &orbit_live_render::Lane) -> bool {
    k.kind == kind::FUNCTION_CALL
        && lane.events().first().is_some_and(|e| e.extra == orbit_live_event::extra::SAMPLED_FRAME)
}

fn is_rail_lane(k: LaneKey) -> bool {
    k.kind == kind::VALUE
}

/// One paint lane per core, 0..N-1, matching native `num_cores_ = max+1`.
/// The machine the scheduler track belongs to. Scheduler lanes are keyed with
/// `pid: 0` (see `LaneKey::scheduler`), which is the local machine.
fn scheduler_machine() -> MachineId {
    MachineId::from_pid(0)
}

/// The Scheduler row and, unless collapsed, one row per core.
fn push_scheduler_rows(
    out: &mut Vec<(RowId, f32)>,
    cores: &[LaneKey],
    strip: &TrackStrip,
    s: f32,
) {
    out.push((RowId::Scheduler, SCHEDULER_H * s));
    if strip.collapsed.contains(&RowId::Scheduler) {
        return;
    }
    for k in cores {
        out.push((RowId::Lane(*k), (lane_height(*k) + lane_gap(*k)) * s));
    }
}

fn scheduler_cores(index: &TrackIndex) -> Vec<LaneKey> {
    let mut n = 0u16;
    for (k, _) in index.lanes() {
        if is_cpu_lane(k) {
            n = n.max(u16::from(k.extra) + 1);
        }
    }
    (0..n).map(|c| LaneKey::scheduler(c as u8)).collect()
}

/// A machine's sort position: the user's order if it has one, else the
/// built-in Local-before-Remote. i64 so an explicit 0..N always sorts ahead
/// of the default key. Free function so a closure sorting one `self` field can
/// capture only `machine_sort`, not all of `self`.
fn machine_rank(machine_sort: &HashMap<MachineId, i32>, m: MachineId) -> i64 {
    machine_sort
        .get(&m)
        .map(|&r| r as i64)
        .unwrap_or(1_000 + m.sort_key() as i64)
}

/// The tier a process sorts into: the target, then everything
/// instrumented, then the service and the viewer's own rows.
fn process_tier(pid: u32, hints: OrderHints) -> u8 {
    if hints.target == Some(pid) {
        TIER_TARGET
    } else if hints.service == Some(pid) || is_self_pid(pid) {
        TIER_AUTO
    } else {
        TIER_INSTRUMENTED
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
    fn multi_process_demo_starts_expanded() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        idx.insert(scope(10, 200, 1));
        idx.insert(scope(11, 300, 1));
        idx.insert(scope(orbit_live_event::dev::VIEWER_PID, 1, 30_000));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert!(!strip.collapsed(RowId::Process(1)));
        assert!(!strip.collapsed(RowId::Process(10)));
        assert!(!strip.collapsed(RowId::Process(11)));
        // The viewer's own rows arrive folded, and last.
        assert!(strip.collapsed(RowId::Process(orbit_live_event::dev::VIEWER_PID)));
        assert_eq!(*strip.process_order.last().unwrap(), orbit_live_event::dev::VIEWER_PID);
        assert!(
            strip.rows().iter().any(|r| matches!(r.id, RowId::Thread(_))),
            "expanded processes must show their threads"
        );
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
        assert!(strip
            .rows()
            .iter()
            .any(|r| r.id == RowId::Machine(MachineId::Local)));
        assert!(!strip
            .rows()
            .iter()
            .any(|r| r.id == RowId::Machine(MachineId::Remote)));
    }

    #[test]
    fn remote_machine_gets_its_own_header() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        idx.insert(scope(orbit_live_event::dev::REMOTE_DEMO_PID, 400, 1));
        idx.insert(scope(orbit_live_event::dev::REMOTE_RENDER_PID, 500, 1));
        idx.insert(scope(orbit_live_event::dev::VIEWER_PID, 1, 30_000));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let ids: Vec<RowId> = strip.rows().iter().map(|r| r.id).collect();
        assert!(ids.contains(&RowId::Machine(MachineId::Local)));
        assert!(ids.contains(&RowId::Machine(MachineId::Remote)));
        let local_i = ids
            .iter()
            .position(|id| *id == RowId::Machine(MachineId::Local))
            .unwrap();
        let remote_i = ids
            .iter()
            .position(|id| *id == RowId::Machine(MachineId::Remote))
            .unwrap();
        assert!(local_i < remote_i, "local machine stays above remote");
        assert!(!strip.collapsed(RowId::Process(1)));
        assert!(!strip.collapsed(RowId::Process(orbit_live_event::dev::REMOTE_DEMO_PID)));
        assert!(strip.collapsed(RowId::Process(orbit_live_event::dev::VIEWER_PID)));
        let viewer_i = ids
            .iter()
            .position(|id| *id == RowId::Process(orbit_live_event::dev::VIEWER_PID))
            .unwrap();
        assert!(viewer_i > local_i && viewer_i < remote_i);
    }

    #[test]
    fn collapse_remote_machine_hides_its_processes() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        idx.insert(scope(orbit_live_event::dev::REMOTE_DEMO_PID, 400, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        strip.toggle(RowId::Machine(MachineId::Remote));
        strip.tick(1.0, &idx, None);
        let ids: Vec<RowId> = strip.rows().iter().map(|r| r.id).collect();
        assert!(ids.contains(&RowId::Machine(MachineId::Remote)));
        assert!(!ids.contains(&RowId::Process(orbit_live_event::dev::REMOTE_DEMO_PID)));
        assert!(ids.contains(&RowId::Process(1)));
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
            *strip.process_order.last().unwrap(),
            orbit_live_event::dev::VIEWER_PID,
            "self-profile processes stay at the bottom of the rail"
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
    fn reorder_process_moves_and_persists() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(10, 1, 1));
        idx.insert(scope(11, 1, 2));
        idx.insert(scope(12, 1, 3));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        assert_eq!(strip.process_order, vec![10, 11, 12]);
        // Move the last process to the front.
        strip.reorder_process(12, 0);
        assert_eq!(strip.process_order, vec![12, 10, 11]);
        // The order survives a rebuild (process_order is re-sorted by the map).
        strip.sync(&idx, None);
        assert_eq!(strip.process_order, vec![12, 10, 11]);
    }

    #[test]
    fn reorder_process_stays_within_rank_tier() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(orbit_live_event::dev::VIEWER_PID, 1, 1));
        idx.insert(scope(10, 1, 2));
        idx.insert(scope(11, 1, 3));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        assert_eq!(*strip.process_order.last().unwrap(), orbit_live_event::dev::VIEWER_PID);
        // Asking a general process to the last slot cannot displace the pinned viewer.
        strip.reorder_process(11, 0);
        assert_eq!(strip.process_order[0], 11);
        assert_eq!(strip.process_order[1], 10);
        assert_eq!(
            strip.process_order[2],
            orbit_live_event::dev::VIEWER_PID,
            "viewer stays pinned below general processes"
        );
    }

    #[test]
    fn the_target_leads_the_instrumented_follow_by_events_and_the_service_folds_last() {
        let mut idx = TrackIndex::default();
        // pid 30 is the target with one scope; 10 said little, 11 a lot;
        // 40 is the service; the viewer's own rows are there too.
        idx.insert(scope(30, 1, 1));
        idx.insert(scope(10, 2, 1));
        for i in 0..8 {
            idx.insert(scope(11, 3, i + 1));
        }
        idx.insert(scope(40, 4, 1));
        idx.insert(scope(orbit_live_event::dev::VIEWER_PID, 1, 30_000));
        let mut strip = TrackStrip::default();
        strip.order_hints = OrderHints { target: Some(30), service: Some(40) };
        strip.sync(&idx, None);
        assert_eq!(strip.process_order[0], 30, "the target first");
        assert_eq!(strip.process_order[1], 11, "then the process that said the most");
        assert_eq!(strip.process_order[2], 10);
        assert_eq!(&strip.process_order[3..], &[40, orbit_live_event::dev::VIEWER_PID][..]);
        assert!(strip.collapsed(RowId::Process(40)), "the service arrives folded");
        assert!(!strip.collapsed(RowId::Process(30)));
        assert!(!strip.collapsed(RowId::Process(11)));
        // An expand by hand is not undone by the next sync.
        strip.toggle(RowId::Process(40));
        strip.sync(&idx, None);
        assert!(!strip.collapsed(RowId::Process(40)));
    }

    #[test]
    fn reorder_machine_moves_and_persists() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(10, 1, 1));
        idx.insert(scope(orbit_live_event::dev::REMOTE_DEMO_PID, 1, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        assert_eq!(
            strip.machines_present(),
            vec![MachineId::Local, MachineId::Remote]
        );
        strip.reorder_machine(MachineId::Remote, 0);
        assert_eq!(
            strip.machines_present(),
            vec![MachineId::Remote, MachineId::Local]
        );
        // Persists across rebuild, and the remote process now sorts first.
        strip.sync(&idx, None);
        assert_eq!(
            strip.machines_present(),
            vec![MachineId::Remote, MachineId::Local]
        );
        assert_eq!(strip.process_order[0], orbit_live_event::dev::REMOTE_DEMO_PID);
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
    fn press_without_moving_leaves_every_row_where_it_was() {
        let mut idx = TrackIndex::default();
        // A layout with everything a real capture has: a scheduler with
        // cores, two processes, threads of unequal height, and VALUE rails
        // (which are laid out as sibling rows, not inside the thread row).
        idx.insert(sched(1, 1, 0, 10, 3));
        for (pid, tid) in [(1u32, 1u32), (1, 2), (1, 3), (9, 4), (9, 5)] {
            idx.insert(ev(kind::API_SCOPE, pid, tid, 0, 0));
            idx.insert(ev(kind::API_SCOPE, pid, tid, 1, 0));
            if tid % 2 == 1 {
                idx.insert(ev(kind::API_SCOPE, pid, tid, 2, 0));
            }
            if tid != 2 {
                idx.insert(ev(kind::VALUE, pid, tid, 0, 0));
            }
            idx.insert(ev(kind::THREAD_STATE, pid, tid, 0, 0));
        }
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let n = strip.thread_order.len();
        assert_eq!(n, 5);
        let before = strip.y.clone();
        let before_h = strip.total_height();
        for k in 0..n {
            let t = strip.thread_order[k];
            let y = strip.y.get(&RowId::Thread(t)).copied().unwrap();
            strip.begin_drag(t, y, y);
            strip.tick(0.0, &idx, None);
            let mut moved: Vec<String> = Vec::new();
            for (id, y0) in &before {
                let y1 = strip.y.get(id).copied().unwrap_or(f32::NAN);
                if (y1 - y0).abs() > 0.01 {
                    moved.push(format!("{id:?} {y0} -> {y1}"));
                }
            }
            assert!(
                moved.is_empty(),
                "press on thread {k} moved rows: {moved:?}"
            );
            assert!(
                (strip.total_height() - before_h).abs() < 0.01,
                "press on thread {k} changed total height {before_h} -> {}",
                strip.total_height()
            );
            strip.end_drag();
            strip.tick(0.0, &idx, None);
        }
    }

    #[test]
    fn press_keeps_body_lanes_under_their_headers() {
        let mut idx = TrackIndex::default();
        idx.insert(sched(1, 1, 0, 10, 3));
        for (pid, tid) in [(1u32, 1u32), (1, 2), (1, 3), (9, 4), (9, 5)] {
            idx.insert(ev(kind::API_SCOPE, pid, tid, 0, 0));
            idx.insert(ev(kind::API_SCOPE, pid, tid, 1, 0));
            if tid % 2 == 1 {
                idx.insert(ev(kind::API_SCOPE, pid, tid, 2, 0));
            }
            if tid != 2 {
                idx.insert(ev(kind::VALUE, pid, tid, 0, 0));
            }
            idx.insert(ev(kind::THREAD_STATE, pid, tid, 0, 0));
        }
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let quiet: std::collections::BTreeMap<LaneKey, i32> = strip
            .layout()
            .iter()
            .map(|(k, y)| (*k, (y * 16.0).round() as i32))
            .collect();
        let mut bad: Vec<String> = Vec::new();
        for t in strip.shown_order() {
            let y = strip.y.get(&RowId::Thread(t)).copied().unwrap();
            strip.begin_drag(t, y, y + 3.0);
            strip.tick(0.0, &idx, None);
            // What the body actually paints while a drag is held: the packed
            // rest plus the lifted thread. Nothing moved, so it must be the
            // same picture as the quiet frame the headers are still drawn from.
            let mut painted: std::collections::BTreeMap<LaneKey, i32> =
                std::collections::BTreeMap::new();
            for (k, y) in strip.rest_layout().into_iter().chain(strip.drag_layout()) {
                painted.insert(k, (y * 16.0).round() as i32);
            }
            for (k, qy) in &quiet {
                match painted.get(k) {
                    None => bad.push(format!("press tid={}: lane {k:?} vanished", t.tid)),
                    Some(py) if py != qy => bad.push(format!(
                        "press tid={}: lane {k:?} moved {} -> {}",
                        t.tid,
                        *qy as f32 / 16.0,
                        *py as f32 / 16.0
                    )),
                    _ => {}
                }
            }
            strip.end_drag();
            strip.tick(0.0, &idx, None);
        }
        bad.truncate(10);
        assert!(bad.is_empty(), "{}", bad.join("\n"));
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

    fn ev(kind_id: u8, pid: u32, tid: u32, depth: u8, extra: u8) -> LiveEvent {
        LiveEvent {
            start_ns: 0,
            duration_ns: 10,
            tid,
            pid,
            kind: kind_id,
            depth,
            extra,
            _pad: 0,
            name_id: 1,
        }
    }

    fn sched(pid: u32, tid: u32, start: u64, dur: u64, core: u8) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid,
            pid,
            kind: kind::SCHEDULING_SLICE,
            depth: 0,
            extra: core,
            _pad: 0,
            name_id: tid,
        }
    }

    #[test]
    fn layout_includes_scheduler_cores_not_as_thread_leaves() {
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::THREAD_STATE, 1, 100, 0, 0));
        idx.insert(ev(kind::SCHEDULING_SLICE, 1, 100, 0, 3));
        idx.insert(ev(kind::API_SCOPE, 1, 100, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 1, 100, 1, 0));
        idx.insert(ev(kind::VALUE, 1, 100, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let sched: Vec<_> = strip
            .layout()
            .iter()
            .filter(|(k, _)| k.kind == kind::SCHEDULING_SLICE)
            .copied()
            .collect();
        assert_eq!(
            sched.len(),
            4,
            "core 3 ⇒ Scheduler (4 cores) with Core 0..3"
        );
        assert!(sched.iter().all(|(k, _)| k.pid == 0 && k.tid == 0));
        assert_eq!(TrackStrip::scheduler_core_count_in(&idx), 4);
        assert!(strip.layout().iter().any(|(k, _)| k.kind == kind::API_SCOPE));
        assert!(strip.layout().iter().any(|(k, _)| k.kind == kind::THREAD_STATE));
        assert!(strip.layout().iter().any(|(k, _)| k.kind == kind::VALUE));
        let row_kinds: Vec<_> = strip
            .rows()
            .iter()
            .filter_map(|r| match r.id {
                RowId::Lane(k) => Some(k.kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            row_kinds,
            vec![
                kind::SCHEDULING_SLICE,
                kind::SCHEDULING_SLICE,
                kind::SCHEDULING_SLICE,
                kind::SCHEDULING_SLICE,
                kind::VALUE
            ]
        );
        assert!(!strip.rows().iter().any(|r| matches!(
            r.id,
            RowId::Lane(k) if k.kind == kind::THREAD_STATE || k.kind == kind::API_SCOPE
        )));
        assert!(strip.rows().iter().any(|r| r.id == RowId::Scheduler));
        // The machine heads the list; the scheduler is the first row under it.
        assert_eq!(strip.rows()[0].id, RowId::Machine(MachineId::Local));
        assert_eq!(strip.rows()[1].id, RowId::Scheduler);
        assert!(strip.rows().iter().any(|r| matches!(r.id, RowId::Thread(_))));
        let th = strip.thread_order[0];
        assert_eq!(th, ThreadId { pid: 1, tid: 100 });
        assert!(!strip.thread_order.iter().any(|t| t.pid == 0 && t.tid == 0));
    }

    #[test]
    fn the_scheduler_lives_under_its_machine_not_beside_it() {
        // Scheduling describes a machine's cores, so it is a child of the
        // machine track rather than a peer of it, and its cores follow.
        let mut idx = TrackIndex::default();
        idx.insert(sched(1, 10, 0, 10, 0));
        idx.insert(sched(1, 11, 0, 10, 1));
        idx.insert(ev(kind::API_SCOPE, 1, 100, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);

        let ids: Vec<RowId> = strip.rows().iter().map(|r| r.id).collect();
        let machine = ids
            .iter()
            .position(|id| matches!(id, RowId::Machine(_)))
            .expect("a machine row");
        let scheduler = ids
            .iter()
            .position(|id| *id == RowId::Scheduler)
            .expect("a scheduler row");
        assert!(machine < scheduler, "scheduler must sit under its machine: {ids:?}");
        // The process for that machine comes after the scheduler's cores.
        let process = ids
            .iter()
            .position(|id| matches!(id, RowId::Process(_)))
            .expect("a process row");
        assert!(scheduler < process, "cores come before processes: {ids:?}");
    }

    #[test]
    fn collapsing_the_machine_hides_its_scheduler() {
        // The test of real nesting: the parent's collapse must take the
        // scheduler and its cores with it.
        let mut idx = TrackIndex::default();
        idx.insert(sched(1, 10, 0, 10, 0));
        idx.insert(sched(1, 11, 0, 10, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert!(strip.rows().iter().any(|r| r.id == RowId::Scheduler));

        strip.toggle(RowId::Machine(MachineId::Local));
        strip.tick(1.0, &idx, None);
        let ids: Vec<RowId> = strip.rows().iter().map(|r| r.id).collect();
        assert!(
            !ids.iter().any(|id| *id == RowId::Scheduler),
            "collapsing the machine must hide the scheduler: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| matches!(id, RowId::Lane(k) if k.is_scheduler())),
            "and its core lanes: {ids:?}"
        );
    }

    #[test]
    fn a_scheduling_only_capture_still_shows_its_machine() {
        // Cores can arrive before any process track exists; the capture must
        // not look empty.
        let mut idx = TrackIndex::default();
        idx.insert(sched(9, 99, 0, 10, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let ids: Vec<RowId> = strip.rows().iter().map(|r| r.id).collect();
        assert_eq!(ids[0], RowId::Machine(MachineId::Local));
        assert_eq!(ids[1], RowId::Scheduler);
    }

    #[test]
    fn n_cores_yield_n_scheduler_lanes() {
        let mut idx = TrackIndex::default();
        idx.insert(sched(1, 10, 0, 10, 0));
        idx.insert(sched(1, 11, 0, 10, 1));
        idx.insert(sched(4, 20, 0, 10, 4));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let cores: Vec<u8> = strip
            .layout()
            .iter()
            .filter(|(k, _)| k.kind == kind::SCHEDULING_SLICE)
            .map(|(k, _)| k.extra)
            .collect();
        assert_eq!(cores, vec![0, 1, 2, 3, 4]);
        assert_eq!(TrackStrip::scheduler_core_count_in(&idx), 5);
        // Scheduling belongs to a machine, so the machine heads the list and
        // the Scheduler row sits under it.
        assert_eq!(strip.rows()[0].id, RowId::Machine(MachineId::Local));
        assert_eq!(strip.rows()[1].id, RowId::Scheduler);
    }

    #[test]
    fn two_threads_on_one_core_share_a_non_overlapping_lane() {
        let mut idx = TrackIndex::default();
        idx.insert(sched(1, 10, 0, 10, 2));
        idx.insert(sched(4, 20, 10, 10, 2));
        idx.insert(ev(kind::API_SCOPE, 1, 10, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 4, 20, 0, 0));
        assert_eq!(
            idx.lanes()
                .filter(|(k, _)| k.kind == kind::SCHEDULING_SLICE)
                .count(),
            1,
            "same core must rebucket into one lane"
        );
        let lane = idx.lane(LaneKey::scheduler(2)).expect("core 2");
        assert_eq!(lane.len(), 2);
        assert!(lane.ends_are_sorted());
        assert!(
            lane.events()[0].end_ns() <= lane.events()[1].start_ns,
            "slices on one core must not overlap"
        );
        assert_eq!(lane.events()[0].tid, 10);
        assert_eq!(lane.events()[1].tid, 20);
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let sched: Vec<_> = strip
            .layout()
            .iter()
            .filter(|(k, _)| k.kind == kind::SCHEDULING_SLICE)
            .collect();
        assert_eq!(sched.len(), 3, "max core 2 ⇒ Core 0..2");
        assert_eq!(
            strip
                .thread_order
                .iter()
                .filter(|t| t.tid == 10 || t.tid == 20)
                .count(),
            2
        );
    }

    #[test]
    fn sync_does_not_invent_a_thread_from_scheduler_keys() {
        let mut idx = TrackIndex::default();
        idx.insert(sched(9, 99, 0, 10, 0));
        idx.insert(sched(9, 98, 10, 10, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert!(
            strip.thread_order.is_empty(),
            "scheduler sentinels must not become process/thread rows"
        );
        assert!(strip.process_order.is_empty());
        assert_eq!(TrackStrip::scheduler_core_count_in(&idx), 2);
        assert_eq!(strip.rows()[0].id, RowId::Machine(MachineId::Local));
        assert_eq!(strip.rows()[1].id, RowId::Scheduler);
        assert!(!strip.rows().iter().any(|r| matches!(r.id, RowId::Thread(_))));
        assert!(!strip.rows().iter().any(|r| matches!(r.id, RowId::Process(_))));
    }

    #[test]
    fn thread_hit_covers_full_block_and_process_chip_restores() {
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::API_SCOPE, 1, 100, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 1, 100, 1, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let th = strip.thread_order[0];
        let (y, h) = strip.thread_band(th).unwrap();
        assert!(h > THREAD_H);
        assert_eq!(strip.hit_at_y(y + THREAD_H + 1.0), Some(RowId::Thread(th)));
        assert_eq!(strip.hit_at_y(y + h - 1.0), Some(RowId::Thread(th)));
        strip.toggle_hidden(th);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert_eq!(strip.hidden_in_process(1), 1);
        strip.show_process_threads(1);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert_eq!(strip.hidden_in_process(1), 0);
        assert!(strip.thread_order.contains(&th));
    }

    /// Per-frame cost of the track layout on a large sampled capture.
    /// Run with `cargo test --release layout_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn layout_bench() {
        // 4 processes x 25 threads; each thread carries a 40-deep flame graph
        // (one lane per depth), a sample bar, and a thread-state bar: the
        // shape a sampled capture of a busy process has.
        let mut idx = TrackIndex::default();
        let mut threads = 0u32;
        for pid in 10..14u32 {
            for t in 0..25u32 {
                let tid = 1000 + pid * 100 + t;
                threads += 1;
                for depth in 0..40u8 {
                    idx.insert(LiveEvent {
                        start_ns: 0,
                        duration_ns: 10,
                        tid,
                        pid,
                        kind: kind::FUNCTION_CALL,
                        depth,
                        extra: 0,
                        _pad: 0,
                        name_id: 1,
                    });
                }
                for k in [kind::SAMPLE, kind::THREAD_STATE] {
                    idx.insert(LiveEvent {
                        start_ns: 0,
                        duration_ns: 10,
                        tid,
                        pid,
                        kind: k,
                        depth: 0,
                        extra: 0,
                        _pad: 0,
                        name_id: 1,
                    });
                }
            }
        }
        let lanes = idx.lane_count();
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let iters = 50;
        let t = std::time::Instant::now();
        for _ in 0..iters {
            strip.sync(&idx, None);
            strip.tick(0.016, &idx, None);
        }
        let frame_ms = t.elapsed().as_secs_f64() * 1e3 / iters as f64;
        // With a process filter, as a live capture of one process runs.
        let t = std::time::Instant::now();
        for _ in 0..iters {
            strip.sync(&idx, Some(11));
            strip.tick(0.016, &idx, Some(11));
        }
        let filtered_ms = t.elapsed().as_secs_f64() * 1e3 / iters as f64;
        strip.sync(&idx, None);
        strip.tick(0.016, &idx, None);
        let total_h = strip.total_height();
        let t = std::time::Instant::now();
        let mut hits = 0usize;
        for i in 0..iters {
            let y = total_h * (i as f32 + 0.5) / iters as f32;
            if strip.hit_at_y(y).is_some() {
                hits += 1;
            }
        }
        let hit_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
        let t = std::time::Instant::now();
        let mut n = 0usize;
        for _ in 0..1_000 {
            n += idx.event_count();
        }
        let count_us = t.elapsed().as_secs_f64() * 1e6 / 1_000.0;
        println!("LAYOUT_BENCH threads={threads} lanes={lanes} rows={}", strip.rows().len());
        println!("LAYOUT_BENCH sync_plus_tick_ms_per_frame={frame_ms:.3}");
        println!("LAYOUT_BENCH sync_plus_tick_filtered_ms_per_frame={filtered_ms:.3}");
        println!("LAYOUT_BENCH hit_at_y_us={hit_us:.1} (hits {hits})");
        println!("LAYOUT_BENCH event_count_us={count_us:.2} (checksum {n})");
    }
}
