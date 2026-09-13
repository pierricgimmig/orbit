//! Machine → process → track session tree.
//!
//! A *track* is the unit the user moves: a thread with the lanes packed under
//! it (states, samples, scopes), a thread's async spans as a block of their
//! own, or one value graph. Each is a `TrackKey`; the rail is one flat
//! `track_order` of them, and a track's position among the others of its
//! process section is its position in that list. A track always belongs to
//! the thread in its key -- that never changes -- but it can be *placed* under
//! another process (`host`), where it is drawn as a guest of that section.

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
    /// The head of a thread's async block: its API_TRACK lanes, every depth,
    /// packed under this row the way scope lanes pack under the thread row.
    Async(ThreadId),
    Lane(LaneKey),
}

/// A movable unit of the rail: what a drag lifts, what a hole is opened for,
/// what can be placed under another process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrackKey {
    /// A thread row with the lanes packed under it.
    Thread(ThreadId),
    /// A thread's async spans: their own block, not part of the thread's.
    Async(ThreadId),
    /// One value graph (one name of one thread).
    Value(LaneKey),
}

impl TrackKey {
    /// The thread the track belongs to. Never changes: moving a track changes
    /// where it is drawn, not whose it is.
    pub fn owner(self) -> ThreadId {
        match self {
            TrackKey::Thread(t) | TrackKey::Async(t) => t,
            TrackKey::Value(k) => ThreadId { pid: k.pid, tid: k.tid },
        }
    }

    pub fn owner_pid(self) -> u32 {
        self.owner().pid
    }

    /// The row that heads the track.
    pub fn row(self) -> RowId {
        match self {
            TrackKey::Thread(t) => RowId::Thread(t),
            TrackKey::Async(t) => RowId::Async(t),
            TrackKey::Value(k) => RowId::Lane(k),
        }
    }

    /// Whether lane `k` is drawn inside this track.
    pub fn owns_lane(self, k: LaneKey) -> bool {
        match self {
            TrackKey::Thread(t) => k.pid == t.pid && k.tid == t.tid && !is_standalone_lane(k),
            TrackKey::Async(t) => k.pid == t.pid && k.tid == t.tid && k.kind == kind::API_TRACK,
            TrackKey::Value(v) => k == v,
        }
    }

    /// Whether an event of `kind_id` on thread (`pid`, `tid`) is drawn inside
    /// this track -- the instance-level twin of [`Self::owns_lane`].
    pub fn owns(self, pid: u32, tid: u32, kind_id: u8) -> bool {
        match self {
            TrackKey::Thread(t) => {
                t.pid == pid && t.tid == tid && kind_id != kind::API_TRACK && kind_id != kind::VALUE
            }
            TrackKey::Async(t) => t.pid == pid && t.tid == tid && kind_id == kind::API_TRACK,
            TrackKey::Value(v) => v.pid == pid && v.tid == tid && kind_id == kind::VALUE,
        }
    }

    /// Whether `row` is the track's head row or a lane row inside it.
    pub fn covers_row(self, row: RowId) -> bool {
        match row {
            RowId::Thread(t) => self == TrackKey::Thread(t),
            RowId::Async(t) => self == TrackKey::Async(t),
            RowId::Lane(k) => !is_cpu_lane(k) && self.owns_lane(k),
            _ => false,
        }
    }
}

impl From<ThreadId> for TrackKey {
    fn from(t: ThreadId) -> Self {
        TrackKey::Thread(t)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TrackRow {
    pub id: RowId,
    pub y: f32,
    pub height: f32,
}

pub struct TrackStrip {
    /// Every track, in rail order across every process. A track's place
    /// among the others shown in its section is its place here; the
    /// sections themselves follow `process_order`.
    pub track_order: Vec<TrackKey>,
    pub process_order: Vec<u32>,
    pub scale: f32,
    /// Where a track is drawn when not under its own process: the pid of
    /// the section it was dropped into. Absent means home. The key still
    /// says whose the track is, so a guest reads as one.
    host: FastMap<TrackKey, u32>,
    collapsed: HashSet<RowId>,
    hidden: HashSet<ThreadId>,
    /// Threads the tracks box hides (see `set_name_filter`), kept apart from
    /// `hidden` -- the user's own list -- so "N hidden / all" keeps counting
    /// only what the user hid, and clearing the box brings nothing back that
    /// the user meant to keep away.
    filtered: HashSet<ThreadId>,
    /// Whether a name filter is on at all. When it is, a machine with nothing
    /// left under it loses its header too, rather than standing empty.
    name_filter: bool,
    /// The scheduler track, judged by the same words against "scheduler".
    scheduler_filtered: bool,
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
    key: TrackKey,
    grab_off: f32,
    pointer_y: f32,
    /// Where the track lands if released now: the section's pid and the
    /// slot among that section's other shown tracks. Refreshed by every
    /// layout while the drag is held; seeded with the track's own place so
    /// a press that never moves drops it where it was.
    dest: (u32, usize),
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
            track_order: Vec::new(),
            process_order: Vec::new(),
            scale: 1.0,
            host: FastMap::default(),
            collapsed: HashSet::new(),
            hidden: HashSet::new(),
            filtered: HashSet::new(),
            name_filter: false,
            scheduler_filtered: false,
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
    pub fn ensure_catalogue(&mut self, index: &TrackIndex) {
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
        // Tracks: what is still there keeps its order and its host; a new
        // thread joins at the end with its async block and its graphs behind
        // it, and a new graph or async block of a known thread joins right
        // after that thread's last track, so it starts out under it.
        let present: FastSet<ThreadId> = threads.iter().copied().collect();
        let catalogue = &self.catalogue;
        self.track_order.retain(|key| {
            let t = key.owner();
            present.contains(&t)
                && match *key {
                    TrackKey::Thread(_) => true,
                    TrackKey::Async(_) => catalogue.leaves_of(t).iter().any(|k| k.kind == kind::API_TRACK),
                    TrackKey::Value(k) => catalogue.leaves_of(t).contains(&k),
                }
        });
        for t in threads {
            let mut want = vec![TrackKey::Thread(t)];
            if self.catalogue.leaves_of(t).iter().any(|k| k.kind == kind::API_TRACK) {
                want.push(TrackKey::Async(t));
            }
            want.extend(
                self.catalogue.leaves_of(t).iter().filter(|k| k.kind == kind::VALUE).map(|k| TrackKey::Value(*k)),
            );
            for key in want {
                if self.track_order.contains(&key) {
                    continue;
                }
                let at = self
                    .track_order
                    .iter()
                    .rposition(|k| k.owner() == t)
                    .map(|i| i + 1)
                    .unwrap_or(self.track_order.len());
                self.track_order.insert(at, key);
            }
        }
        // A host that left the rail sends its guests home.
        let order = &self.track_order;
        let listed = &self.process_order;
        self.host.retain(|key, pid| order.contains(key) && listed.contains(pid) && *pid != key.owner_pid());
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
        !self.hidden.contains(&t) && !self.filtered.contains(&t)
    }

    /// Applies the tracks box. `filtered` is the set of threads the words do
    /// not match, `None` when the box is empty; `scheduler_filtered` says the
    /// words do not match the scheduler track either. The layout is only
    /// invalidated when something actually changed, so calling this every
    /// frame with the same answer costs a few comparisons.
    pub fn set_name_filter(&mut self, filtered: Option<HashSet<ThreadId>>, scheduler_filtered: bool) {
        let active = filtered.is_some();
        let filtered = filtered.unwrap_or_default();
        if active == self.name_filter && filtered == self.filtered && scheduler_filtered == self.scheduler_filtered {
            return;
        }
        self.name_filter = active;
        self.filtered = filtered;
        self.scheduler_filtered = scheduler_filtered;
        self.layout_gen = self.layout_gen.wrapping_add(1);
    }

    /// How many threads the tracks box is hiding right now.
    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    /// Every thread the index knows, in first-seen order -- the population a
    /// name filter is judged over. Call [`Self::ensure_catalogue`] first.
    pub fn catalogue_threads(&self) -> &[ThreadId] {
        &self.catalogue.threads
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn is_dragging_thread(&self, t: ThreadId) -> bool {
        self.dragging_track() == Some(TrackKey::Thread(t))
    }

    pub fn is_dragging(&self, key: TrackKey) -> bool {
        self.dragging_track() == Some(key)
    }

    /// The thread being dragged, when the lifted track is a thread.
    pub fn dragging_thread(&self) -> Option<ThreadId> {
        match self.dragging_track() {
            Some(TrackKey::Thread(t)) => Some(t),
            _ => None,
        }
    }

    pub fn dragging_track(&self) -> Option<TrackKey> {
        self.drag.as_ref().map(|d| d.key)
    }

    /// The threads in rail order, shown or not.
    pub fn thread_order(&self) -> Vec<ThreadId> {
        self.track_order
            .iter()
            .filter_map(|k| match k {
                TrackKey::Thread(t) => Some(*t),
                _ => None,
            })
            .collect()
    }

    /// The section a track is drawn in: its host when it was moved, else
    /// its own process. A host that is not on the rail counts as home.
    pub fn host_pid(&self, key: TrackKey) -> u32 {
        self.host
            .get(&key)
            .copied()
            .filter(|p| self.process_order.contains(p))
            .unwrap_or(key.owner_pid())
    }

    /// Whether the track is drawn under a process that is not its own.
    pub fn is_guest(&self, key: TrackKey) -> bool {
        self.host_pid(key) != key.owner_pid()
    }

    /// How many guests a section is showing.
    pub fn guests_in_process(&self, pid: u32) -> usize {
        self.track_order
            .iter()
            .filter(|k| k.owner_pid() != pid && self.host_pid(**k) == pid && self.shown(**k))
            .count()
    }

    /// Puts a moved track back under its own process, after its thread's
    /// other tracks so a graph lands under its thread again.
    pub fn send_home(&mut self, key: TrackKey) {
        self.host.remove(&key);
        let Some(cur) = self.track_order.iter().position(|k| *k == key) else {
            return;
        };
        self.track_order.remove(cur);
        let owner = key.owner();
        let home = owner.pid;
        let at = self
            .track_order
            .iter()
            .rposition(|k| k.owner() == owner && self.host_pid(*k) == home)
            .or_else(|| self.track_order.iter().rposition(|k| self.host_pid(*k) == home))
            .map(|i| i + 1)
            .unwrap_or(self.track_order.len());
        self.track_order.insert(at, key);
        self.layout_gen = self.layout_gen.wrapping_add(1);
    }

    /// Places `key` at `slot` among the shown tracks of section `pid` --
    /// its own process or another one. The shown tracks of the section are
    /// permuted over the positions they already hold, so a hidden track
    /// keeps its place in the list and comes back where it was; a track
    /// arriving from another section takes a new position after them.
    pub fn place(&mut self, key: TrackKey, pid: u32, slot: usize) {
        let was_here = self.host_pid(key) == pid;
        let mut slots: Vec<usize> = self
            .track_order
            .iter()
            .enumerate()
            .filter(|(_, k)| self.host_pid(**k) == pid && self.shown(**k))
            .map(|(i, _)| i)
            .collect();
        let mut shown: Vec<TrackKey> =
            slots.iter().map(|i| self.track_order[*i]).filter(|k| *k != key).collect();
        shown.insert(slot.min(shown.len()), key);
        if !was_here {
            if let Some(cur) = self.track_order.iter().position(|k| *k == key) {
                self.track_order.remove(cur);
                for i in slots.iter_mut() {
                    if *i > cur {
                        *i -= 1;
                    }
                }
            }
            let at = slots
                .last()
                .map(|l| l + 1)
                .or_else(|| self.track_order.iter().rposition(|k| self.host_pid(*k) == pid).map(|i| i + 1))
                .unwrap_or(self.track_order.len());
            self.track_order.insert(at, key);
            slots.push(at);
        }
        for (i, k) in slots.into_iter().zip(shown) {
            self.track_order[i] = k;
        }
        if pid == key.owner_pid() {
            self.host.remove(&key);
        } else {
            self.host.insert(key, pid);
        }
        // A track dropped into a folded section would vanish into it;
        // unfold it, and count that as the user's doing so the auto-fold of
        // the service's rows does not close it again.
        if self.collapsed.remove(&RowId::Process(pid)) {
            self.user_toggled.insert(RowId::Process(pid));
        }
        self.layout_gen = self.layout_gen.wrapping_add(1);
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

    /// Whether `row` is on thread `t`'s own track (its header or a lane
    /// packed under it). Its graphs and async block are tracks of their own.
    pub fn row_on_thread(row: RowId, t: ThreadId) -> bool {
        TrackKey::Thread(t).covers_row(row)
    }

    pub fn row_on_track(row: RowId, key: TrackKey) -> bool {
        key.covers_row(row)
    }

    /// A track shows when its thread does: hiding or filtering a thread
    /// takes its graphs and async block with it, wherever they were moved.
    fn shown(&self, key: TrackKey) -> bool {
        self.is_shown(key.owner())
    }

    /// Threads in rail order, the shown ones.
    #[cfg(test)]
    fn shown_order(&self) -> Vec<ThreadId> {
        self.track_order
            .iter()
            .filter_map(|k| match k {
                TrackKey::Thread(t) if self.is_shown(*t) => Some(*t),
                _ => None,
            })
            .collect()
    }

    /// The shown tracks drawn in section `pid`, in order, without `exclude`
    /// (the one being dragged, which floats instead).
    fn section_items(&self, pid: u32, exclude: Option<TrackKey>) -> Vec<TrackKey> {
        self.track_order
            .iter()
            .copied()
            .filter(|k| Some(*k) != exclude && self.shown(*k) && self.host_pid(*k) == pid)
            .collect()
    }

    /// A process keeps its header while it shows any track or owns any
    /// shown one, so a section emptied by moving its tracks away is still
    /// there to drop them back into.
    fn process_is_listed(&self, pid: u32) -> bool {
        if let Some(fp) = self.filter_pid {
            if fp != pid && !is_self_pid(pid) {
                return false;
            }
        }
        self.track_order
            .iter()
            .any(|k| self.shown(*k) && (self.host_pid(*k) == pid || k.owner_pid() == pid))
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

    /// Where the dragged track lands if released now, read off the rows as
    /// they were drawn last frame -- hole included, since that is what the
    /// pointer is over: the section whose header is the last one above the
    /// lifted track's top, and the slot among that section's tracks whose
    /// midpoints are above it. Any section will do -- that is how a track
    /// crosses into another process. Stable frame to frame: once the hole
    /// passes a row that row shifts by the hole's height, away from the
    /// pointer, so the comparison does not flip back.
    fn drop_target(&self) -> (u32, usize) {
        let Some(d) = &self.drag else {
            return (0, 0);
        };
        let home = (d.key.owner_pid(), 0);
        if !self.shown(d.key) {
            return home;
        }
        let header_top = d.pointer_y - d.grab_off;
        let mut sections: Vec<(u32, f32, usize)> = Vec::new();
        for row in &self.cached_rows {
            match row.id {
                RowId::Process(p) => sections.push((p, row.y, 0)),
                RowId::Machine(_) | RowId::Scheduler => {}
                RowId::Lane(k) if is_cpu_lane(k) => {}
                RowId::Thread(_) | RowId::Async(_) | RowId::Lane(_) => {
                    if d.key.covers_row(row.id) {
                        continue;
                    }
                    if let Some(section) = sections.last_mut() {
                        if row.y + row.height * 0.5 < header_top {
                            section.2 += 1;
                        }
                    }
                }
            }
        }
        let Some(first) = sections.first() else {
            return home;
        };
        let mut target = *first;
        for s in &sections {
            if s.1 <= header_top {
                target = *s;
            }
        }
        (target.0, target.2)
    }

    /// The height of a track's block as drawn at rest.
    fn block_h(&self, key: TrackKey) -> f32 {
        match key {
            TrackKey::Thread(t) => self.thread_scope_stack_h(t),
            TrackKey::Async(t) => self.async_block_h(t),
            TrackKey::Value(k) => (lane_height(k) + lane_gap(k)) * self.scale.max(0.01),
        }
    }

    /// A thread's async lanes, every depth, stacked.
    fn async_block_h(&self, t: ThreadId) -> f32 {
        let s = self.scale.max(0.01);
        self.catalogue
            .leaves_of(t)
            .iter()
            .filter(|k| k.kind == kind::API_TRACK)
            .map(|k| (lane_height(*k) + lane_gap(*k)) * s)
            .sum()
    }

    /// The thread row with the lanes packed under it. Its graphs and async
    /// block are separate tracks and do not count.
    pub fn thread_block_h(&self, t: ThreadId) -> f32 {
        self.thread_scope_stack_h(t)
    }

    pub fn hidden_in_process(&self, pid: u32) -> usize {
        self.hidden.iter().filter(|t| t.pid == pid).count()
    }

    pub fn show_process_threads(&mut self, pid: u32) {
        self.hidden.retain(|t| t.pid != pid);
        self.layout_gen = self.layout_gen.wrapping_add(1);
    }

    /// Hit test: the row at `y`. A thread's and an async block's rows span
    /// the lanes packed under them, so a hit anywhere in the block is the
    /// block's head.
    pub fn hit_at_y(&self, y: f32) -> Option<RowId> {
        self.row_at_y(y)
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
        debug_assert_eq!(self.catalogue.gen, Some(index.lane_gen()), "catalogue is stale");
        let exclude = self.drag.as_ref().map(|d| d.key);
        let dest = self.drop_target();
        let skeleton = self.skeleton(exclude);
        if let Some(d) = &mut self.drag {
            d.dest = dest;
        }
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
        // The lifted track floats under the pointer: its head row at the
        // grab point, its lanes packed under it from the same catalogue, in
        // the same order, as every resting track's (reading `index.lanes()`
        // here once drew sampled-frame lanes on the lifted thread that no
        // resting thread shows).
        if let Some(d) = &self.drag {
            if self.shown(d.key) {
                next.insert(d.key.row(), d.pointer_y - d.grab_off);
            }
        }
        self.assign_packed_leaf_ys(&mut next);
        if next != self.y {
            self.layout_gen = self.layout_gen.wrapping_add(1);
        }
        self.y = next;
        self.rebuild_rows();
        if let (Some(hy), Some(d)) = (hole_y, self.drag.as_ref()) {
            let hole_h = self.block_h(d.key);
            self.cached_total_h = self.cached_total_h.max(hy + hole_h);
        }
    }

    /// The lanes packed under each thread row and each async row, at rest
    /// or lifted alike: they follow their head row's Y.
    fn assign_packed_leaf_ys(&self, next: &mut FastMap<RowId, f32>) {
        let s = self.scale.max(0.01);
        for key in &self.track_order {
            match *key {
                TrackKey::Thread(t) => {
                    if self.collapsed.contains(&RowId::Thread(t)) {
                        continue;
                    }
                    let Some(&ty) = next.get(&RowId::Thread(t)) else {
                        continue;
                    };
                    let mut ly = ty + THREAD_H * s;
                    for &k in self.catalogue.leaves_of(t) {
                        if is_standalone_lane(k) {
                            continue;
                        }
                        next.insert(RowId::Lane(k), ly);
                        ly += (lane_height(k) + lane_gap(k)) * s;
                    }
                }
                TrackKey::Async(t) => {
                    let Some(&ty) = next.get(&RowId::Async(t)) else {
                        continue;
                    };
                    let mut ly = ty;
                    for &k in self.catalogue.leaves_of(t) {
                        if k.kind != kind::API_TRACK {
                            continue;
                        }
                        next.insert(RowId::Lane(k), ly);
                        ly += (lane_height(k) + lane_gap(k)) * s;
                    }
                }
                TrackKey::Value(_) => {}
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
                // Lanes packed under a thread or async row have a Y but no
                // row of their own; graphs and cores are rows.
                if k.kind != kind::VALUE && !is_cpu_lane(k) {
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

    /// Packed rest lanes (no dragged track). Background raster / instance Ys.
    pub fn rest_layout(&self) -> Vec<(LaneKey, f32)> {
        let Some(d) = &self.drag else {
            return self.cached_layout.clone();
        };
        self.cached_layout
            .iter()
            .copied()
            .filter(|(k, _)| !d.key.owns_lane(*k))
            .collect()
    }

    /// The dragged track's lanes at the floating pointer Y.
    pub fn drag_layout(&self) -> Vec<(LaneKey, f32)> {
        let Some(d) = &self.drag else {
            return Vec::new();
        };
        self.cached_layout
            .iter()
            .copied()
            .filter(|(k, _)| d.key.owns_lane(*k))
            .collect()
    }

    /// Lifts a track. `lane_y` is its head row's rail Y, `pointer_y` the
    /// pointer's, so the grab point stays under the pointer.
    pub fn begin_drag(&mut self, key: impl Into<TrackKey>, lane_y: f32, pointer_y: f32) {
        let key = key.into();
        let pid = self.host_pid(key);
        let slot = self
            .section_items(pid, None)
            .iter()
            .position(|k| *k == key)
            .unwrap_or(0);
        self.drag = Some(Drag {
            key,
            grab_off: pointer_y - lane_y,
            pointer_y,
            dest: (pid, slot),
        });
    }

    pub fn update_drag(&mut self, pointer_y: f32) {
        if let Some(d) = &mut self.drag {
            d.pointer_y = pointer_y;
        }
    }

    /// Drops the track where the pointer is now, judged against the rows as
    /// last drawn, so a release with no frame in between still lands where
    /// it points.
    pub fn end_drag(&mut self) {
        let Some(key) = self.drag.as_ref().map(|d| d.key) else {
            return;
        };
        let dest = self.drop_target();
        self.drag = None;
        if self.shown(key) {
            let (pid, slot) = dest;
            self.place(key, pid, slot);
        }
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
            if !is_standalone_lane(k) {
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
            RowId::Async(t) => self.async_block_h(t),
            RowId::Lane(k) => (lane_height(k) + lane_gap(k)) * s,
        }
    }

    /// The resting skeleton with a hole the size of the dragged track opened
    /// at `dest`: before the `slot`-th track of section `pid`, or at the end
    /// of that section when it has fewer -- right under the header of a
    /// collapsed or emptied one.
    fn skeleton_with_hole(&self, skeleton: &[(RowId, f32)], dest: (u32, usize)) -> Vec<SkelItem> {
        let plain = || skeleton.iter().map(|(id, h)| SkelItem::Row(*id, *h)).collect::<Vec<_>>();
        let Some(d) = &self.drag else {
            return plain();
        };
        if !self.shown(d.key) {
            return plain();
        }
        let hole = SkelItem::Hole(self.block_h(d.key));
        let (pid, slot) = dest;
        let mut items: Vec<SkelItem> = Vec::with_capacity(skeleton.len() + 1);
        let mut current: Option<u32> = None;
        let mut seen = 0usize;
        let mut inserted = false;
        for &(id, h) in skeleton {
            let leaving = match id {
                RowId::Process(_) | RowId::Machine(_) | RowId::Scheduler => true,
                _ => false,
            };
            if leaving && current == Some(pid) && !inserted {
                items.push(SkelItem::Hole(self.block_h(d.key)));
                inserted = true;
            }
            match id {
                RowId::Process(p) => {
                    current = Some(p);
                    seen = 0;
                }
                RowId::Machine(_) | RowId::Scheduler => current = None,
                RowId::Lane(k) if is_cpu_lane(k) => {}
                _ if current == Some(pid) => {
                    if seen == slot && !inserted {
                        items.push(SkelItem::Hole(self.block_h(d.key)));
                        inserted = true;
                    }
                    seen += 1;
                }
                _ => {}
            }
            items.push(SkelItem::Row(id, h));
        }
        if !inserted {
            items.push(hole);
        }
        items
    }

    /// The rail at rest: every row and its height, top to bottom, without
    /// `exclude` (the track being dragged, which floats instead).
    fn skeleton(&self, exclude: Option<TrackKey>) -> Vec<(RowId, f32)> {
        let s = self.scale.max(0.01);
        let filter_pid = self.filter_pid;
        let mut out = Vec::new();
        let cores = &self.catalogue.cores;
        // The scheduler describes a machine's cores, so it belongs under that
        // machine rather than beside it. It is emitted inside the machine loop
        // below; `scheduler_machine` says which machine owns it.
        if self.track_order.is_empty() && self.process_order.is_empty() {
            // Still show the scheduler when a capture has cores but no
            // process tracks yet -- otherwise a scheduling-only capture looks
            // empty.
            if !cores.is_empty() && !self.scheduler_filtered {
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
            if self.collapsed.contains(&RowId::Machine(m)) {
                out.push((RowId::Machine(m), MACHINE_H * s));
                continue;
            }
            // The machine's rows are gathered first: with a name filter on, a
            // machine that has nothing left under it loses its header too.
            let mut section: Vec<(RowId, f32)> = Vec::new();
            if m == scheduler_owner && !cores.is_empty() && !self.scheduler_filtered {
                push_scheduler_rows(&mut section, cores, self, s);
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
                if !self.process_is_listed(pid) {
                    continue;
                }
                section.push((RowId::Process(pid), PROCESS_H * s));
                if self.collapsed.contains(&RowId::Process(pid)) {
                    continue;
                }
                for key in self.section_items(pid, exclude) {
                    section.push((key.row(), self.block_h(key)));
                }
            }
            if self.name_filter && section.is_empty() {
                continue;
            }
            out.push((RowId::Machine(m), MACHINE_H * s));
            out.extend(section);
        }
        out
    }
}

/// The words of a track filter: lower-cased, split on whitespace. Empty for a
/// blank box, which means no filter.
pub fn filter_tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a track's label matches the words: it contains at least one of
/// them. Any word rather than every word, as C++ Orbit's TrackManager
/// filtered, so "physics render" shows both families of threads. `label`
/// must already be lower-case; no words matches everything.
pub fn label_matches(label: &str, tokens: &[String]) -> bool {
    tokens.is_empty() || tokens.iter().any(|t| label.contains(t.as_str()))
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

/// A lane that is a track of its own rather than packed under its thread:
/// a value graph, or an async lane (part of the thread's async block).
fn is_standalone_lane(k: LaneKey) -> bool {
    k.kind == kind::VALUE || k.kind == kind::API_TRACK
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
    fn filter_words_are_lower_cased_and_any_one_matches() {
        assert!(filter_tokens("  ").is_empty());
        assert_eq!(filter_tokens("Physics  RENDER"), vec!["physics", "render"]);
        let words = filter_tokens("physics zzzz");
        assert!(label_matches("7 physics-2 1234 orbittestrust", &words));
        assert!(!label_matches("8 render 1234 orbittestrust", &words));
        assert!(label_matches("anything", &[]));
    }

    #[test]
    fn name_filter_hides_threads_then_processes_then_the_machine() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 100, 1));
        idx.insert(scope(1, 101, 1));
        idx.insert(scope(4, 200, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let rows = |strip: &TrackStrip| strip.rows().iter().map(|r| r.id).collect::<Vec<_>>();
        let all = rows(&strip);
        assert!(all.contains(&RowId::Thread(ThreadId { pid: 1, tid: 101 })));
        // One thread filtered: it goes, its process stays.
        let one: HashSet<ThreadId> = [ThreadId { pid: 1, tid: 101 }].into_iter().collect();
        strip.set_name_filter(Some(one), false);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let r = rows(&strip);
        assert!(!r.contains(&RowId::Thread(ThreadId { pid: 1, tid: 101 })));
        assert!(r.contains(&RowId::Thread(ThreadId { pid: 1, tid: 100 })));
        assert!(r.contains(&RowId::Process(1)));
        assert_eq!(strip.filtered_count(), 1);
        assert_eq!(strip.hidden_count(), 0, "the user's own hidden list is untouched");
        // Every thread of a process filtered: the process header goes too.
        let proc1: HashSet<ThreadId> = [ThreadId { pid: 1, tid: 100 }, ThreadId { pid: 1, tid: 101 }].into_iter().collect();
        strip.set_name_filter(Some(proc1), false);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let r = rows(&strip);
        assert!(!r.contains(&RowId::Process(1)));
        assert!(r.contains(&RowId::Process(4)));
        // Everything filtered: the machine header goes as well.
        let every: HashSet<ThreadId> = strip.catalogue_threads().iter().copied().collect();
        strip.set_name_filter(Some(every), true);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert!(rows(&strip).is_empty(), "nothing matched, nothing drawn: {:?}", rows(&strip));
        // Clearing the box restores every row.
        strip.set_name_filter(None, false);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        assert_eq!(rows(&strip), all);
    }

    #[test]
    fn same_tid_different_pid_are_separate_threads() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 7, 1));
        idx.insert(scope(4, 7, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        assert_eq!(strip.process_order, vec![1, 4]);
        assert_eq!(strip.thread_order().len(), 2);
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
        let th = strip.thread_order()[0];
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
        let first = strip.thread_order()[0];
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
        let first = strip.thread_order()[0];
        let second = strip.thread_order()[1];
        let y0 = strip.y.get(&RowId::Thread(first)).copied().unwrap_or(0.0);
        strip.begin_drag(first, y0, y0);
        strip.update_drag(y0 + 80.0);
        strip.end_drag();
        assert_eq!(strip.thread_order()[0], second);
        assert_eq!(strip.thread_order()[1], first);
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
        let first = strip.thread_order()[0];
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
        let t0 = strip.thread_order()[0];
        let t1 = strip.thread_order()[1];
        let t2 = strip.thread_order()[2];
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
        let n = strip.thread_order().len();
        assert_eq!(n, 5);
        let before = strip.y.clone();
        let before_h = strip.total_height();
        for k in 0..n {
            let t = strip.thread_order()[k];
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
    fn a_dragged_thread_never_lifts_its_sampled_frame_lanes() {
        // The regression behind blog post 20's sibling bug: the dragged
        // thread's lanes were once collected straight from the index, which
        // (unlike the catalogue every resting thread uses) does not drop the
        // sampled-frame flame. The two paths must select the same lanes.
        let mut idx = TrackIndex::default();
        // A thread with a real scope lane, a value rail, a thread-state lane,
        // and a lane of sampled callstack frames (FUNCTION_CALL / SAMPLED_FRAME).
        idx.insert(ev(kind::API_SCOPE, 1, 1, 0, 0));
        idx.insert(ev(kind::VALUE, 1, 1, 0, 0));
        idx.insert(ev(kind::THREAD_STATE, 1, 1, 0, 0));
        idx.insert(ev(kind::FUNCTION_CALL, 1, 1, 0, orbit_live_event::extra::SAMPLED_FRAME));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let sampled: Vec<LaneKey> = strip
            .layout()
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| k.kind == kind::FUNCTION_CALL)
            .collect();
        assert!(sampled.is_empty(), "resting layout already draws sampled frames: {sampled:?}");
        let t = ThreadId { pid: 1, tid: 1 };
        let y = strip.y.get(&RowId::Thread(t)).copied().unwrap();
        strip.begin_drag(t, y, y + 40.0);
        strip.tick(0.0, &idx, None);
        let lifted = strip.drag_layout();
        assert!(
            lifted.iter().all(|(k, _)| k.kind != kind::FUNCTION_CALL),
            "the dragged thread lifted a sampled-frame lane: {lifted:?}"
        );
        // And it still carries the lanes it should: the scope and the state.
        // The graph is a track of its own now and stays at rest.
        for want in [kind::API_SCOPE, kind::THREAD_STATE] {
            assert!(
                lifted.iter().any(|(k, _)| k.kind == want),
                "the dragged thread dropped a {want} lane: {lifted:?}"
            );
        }
        assert!(
            lifted.iter().all(|(k, _)| k.kind != kind::VALUE),
            "the graph is its own track and must not lift with the thread: {lifted:?}"
        );
        assert!(strip.rest_layout().iter().any(|(k, _)| k.kind == kind::VALUE));
        strip.end_drag();
    }

    fn row_ids(strip: &TrackStrip) -> Vec<RowId> {
        strip.rows().iter().map(|r| r.id).collect()
    }

    #[test]
    fn graphs_and_async_blocks_are_tracks_of_their_own_under_their_thread() {
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::API_SCOPE, 1, 1, 0, 0));
        idx.insert(ev(kind::API_TRACK, 1, 1, 0, 0));
        idx.insert(ev(kind::API_TRACK, 1, 1, 1, 0));
        idx.insert(ev(kind::VALUE, 1, 1, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 1, 2, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let t1 = ThreadId { pid: 1, tid: 1 };
        let graph = LaneKey { pid: 1, tid: 1, kind: kind::VALUE, depth: 1, extra: 0 };
        assert_eq!(
            strip.track_order,
            vec![
                TrackKey::Thread(t1),
                TrackKey::Async(t1),
                TrackKey::Value(graph),
                TrackKey::Thread(ThreadId { pid: 1, tid: 2 })
            ],
            "a thread, then its async block and its graph, then the next thread"
        );
        let ids = row_ids(&strip);
        let at = |id: RowId| ids.iter().position(|i| *i == id).unwrap_or_else(|| panic!("{id:?} in {ids:?}"));
        assert!(at(RowId::Thread(t1)) < at(RowId::Async(t1)));
        assert!(at(RowId::Async(t1)) < at(RowId::Lane(graph)));
        assert!(
            !ids.iter().any(|id| matches!(id, RowId::Lane(k) if k.kind == kind::API_TRACK)),
            "async lanes pack under their block's head row, not as rows: {ids:?}"
        );
        // The async block is as tall as its two depth lanes, and the thread
        // row no longer counts them.
        let async_h = strip.rows().iter().find(|r| r.id == RowId::Async(t1)).unwrap().height;
        let lane = LaneKey { pid: 1, tid: 1, kind: kind::API_TRACK, depth: 0, extra: 0 };
        assert!((async_h - 2.0 * (lane_height(lane) + lane_gap(lane))).abs() < 0.01);
        let (_, thread_h) = strip.thread_band(t1).unwrap();
        let scope = LaneKey { pid: 1, tid: 1, kind: kind::API_SCOPE, depth: 0, extra: 0 };
        assert!((thread_h - (THREAD_H + lane_height(scope) + lane_gap(scope))).abs() < 0.01);
        // Both async lanes have a Y inside the block.
        let ay = strip.y[&RowId::Async(t1)];
        let d0 = strip.layout().iter().find(|(k, _)| *k == lane).unwrap().1;
        let d1 = strip.layout().iter().find(|(k, _)| k.kind == kind::API_TRACK && k.depth == 1).unwrap().1;
        assert!((d0 - ay).abs() < 0.01 && d1 > d0 && d1 < ay + async_h);
        // Hovering inside the block hits its head.
        assert_eq!(strip.hit_at_y(ay + async_h - 1.0), Some(RowId::Async(t1)));
    }

    #[test]
    fn a_graph_drags_on_its_own_and_lifts_nothing_else() {
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::API_SCOPE, 1, 1, 0, 0));
        idx.insert(ev(kind::VALUE, 1, 1, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 1, 2, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let t1 = ThreadId { pid: 1, tid: 1 };
        let t2 = ThreadId { pid: 1, tid: 2 };
        let graph = LaneKey { pid: 1, tid: 1, kind: kind::VALUE, depth: 1, extra: 0 };
        let gy = strip.y[&RowId::Lane(graph)];
        let before = strip.y.clone();
        strip.begin_drag(TrackKey::Value(graph), gy, gy + 5.0);
        strip.tick(0.0, &idx, None);
        assert_eq!(strip.dragging_track(), Some(TrackKey::Value(graph)));
        assert!(strip.dragging_thread().is_none(), "a graph drag is not a thread drag");
        assert_eq!(strip.drag_layout().iter().map(|(k, _)| *k).collect::<Vec<_>>(), vec![graph]);
        assert!(strip.rest_layout().iter().any(|(k, _)| k.kind == kind::API_SCOPE && k.tid == 1));
        // A press that has not moved leaves every row where it was.
        for (id, y0) in &before {
            let y1 = strip.y[id];
            assert!((y1 - y0).abs() < 0.01, "{id:?} moved {y0} -> {y1} on a press");
        }
        // Dragged past the second thread it lands after it, on its own.
        strip.update_drag(gy + 5.0 + 200.0);
        strip.tick(0.0, &idx, None);
        strip.end_drag();
        assert_eq!(
            strip.track_order,
            vec![TrackKey::Thread(t1), TrackKey::Thread(t2), TrackKey::Value(graph)]
        );
        strip.tick(0.0, &idx, None);
        let ids = row_ids(&strip);
        assert_eq!(ids.last(), Some(&RowId::Lane(graph)));
        // A sync keeps the user's order.
        strip.sync(&idx, None);
        assert_eq!(strip.track_order[2], TrackKey::Value(graph));
    }

    #[test]
    fn a_track_moves_into_another_process_as_a_guest_and_can_go_home() {
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::API_SCOPE, 1, 1, 0, 0));
        idx.insert(ev(kind::VALUE, 1, 1, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 4, 7, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let graph = LaneKey { pid: 1, tid: 1, kind: kind::VALUE, depth: 1, extra: 0 };
        let key = TrackKey::Value(graph);
        assert!(!strip.is_guest(key));
        // Drag the graph below process 4's thread.
        let gy = strip.y[&RowId::Lane(graph)];
        let far = strip.total_height() + 40.0;
        strip.begin_drag(key, gy, gy);
        strip.update_drag(far);
        strip.tick(0.0, &idx, None);
        strip.end_drag();
        strip.tick(0.0, &idx, None);
        assert!(strip.is_guest(key), "the graph is now a guest of process 4");
        assert_eq!(strip.host_pid(key), 4);
        assert_eq!(key.owner_pid(), 1, "and still process 1's");
        assert_eq!(strip.guests_in_process(4), 1);
        let ids = row_ids(&strip);
        let p4 = ids.iter().position(|i| *i == RowId::Process(4)).unwrap();
        let g = ids.iter().position(|i| *i == RowId::Lane(graph)).unwrap();
        assert!(g > p4, "drawn in process 4's section: {ids:?}");
        assert!(ids.contains(&RowId::Process(1)), "process 1 keeps its header");
        // The placement survives a sync.
        strip.sync(&idx, None);
        strip.tick(0.0, &idx, None);
        assert_eq!(strip.host_pid(key), 4);
        // Home again: under its own thread.
        strip.send_home(key);
        strip.tick(0.0, &idx, None);
        assert!(!strip.is_guest(key));
        let ids = row_ids(&strip);
        let t1 = ids.iter().position(|i| *i == RowId::Thread(ThreadId { pid: 1, tid: 1 })).unwrap();
        let g = ids.iter().position(|i| *i == RowId::Lane(graph)).unwrap();
        let p4 = ids.iter().position(|i| *i == RowId::Process(4)).unwrap();
        assert!(t1 < g && g < p4, "{ids:?}");
    }

    #[test]
    fn a_whole_thread_can_be_a_guest_and_its_section_stays_to_take_it_back() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(4, 7, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let t1 = ThreadId { pid: 1, tid: 1 };
        // Process 1's only thread goes under process 4: process 1 is empty
        // but still listed, and process 4 shows both threads.
        strip.place(TrackKey::Thread(t1), 4, 0);
        strip.tick(0.0, &idx, None);
        let ids = row_ids(&strip);
        assert!(ids.contains(&RowId::Process(1)), "{ids:?}");
        let p4 = ids.iter().position(|i| *i == RowId::Process(4)).unwrap();
        assert_eq!(ids[p4 + 1], RowId::Thread(t1), "slot 0 of process 4: {ids:?}");
        assert_eq!(ids[p4 + 2], RowId::Thread(ThreadId { pid: 4, tid: 7 }));
        assert!(strip.is_guest(TrackKey::Thread(t1)));
        // Hiding the thread hides it wherever it is; showing its process's
        // threads brings it back, still a guest.
        strip.toggle_hidden(t1);
        strip.tick(0.0, &idx, None);
        assert!(!row_ids(&strip).contains(&RowId::Thread(t1)));
        assert_eq!(strip.hidden_in_process(1), 1, "hidden counts against its own process");
        strip.show_process_threads(1);
        strip.tick(0.0, &idx, None);
        assert!(row_ids(&strip).contains(&RowId::Thread(t1)));
        assert!(strip.is_guest(TrackKey::Thread(t1)));
    }

    #[test]
    fn a_host_that_leaves_the_rail_sends_its_guests_home() {
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::API_SCOPE, 1, 1, 0, 0));
        idx.insert(ev(kind::VALUE, 1, 1, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 4, 7, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let graph = LaneKey { pid: 1, tid: 1, kind: kind::VALUE, depth: 1, extra: 0 };
        strip.place(TrackKey::Value(graph), 4, 0);
        assert!(strip.is_guest(TrackKey::Value(graph)));
        // A capture filter narrows the rail to process 1: process 4 goes,
        // and the graph is home again rather than lost with it.
        strip.sync(&idx, Some(1));
        strip.tick(0.0, &idx, Some(1));
        assert!(!strip.process_order.contains(&4));
        assert!(!strip.is_guest(TrackKey::Value(graph)));
        assert!(row_ids(&strip).contains(&RowId::Lane(graph)));
    }

    #[test]
    fn a_drag_past_a_middle_section_settles_where_the_pointer_is() {
        // Three sections; the pointer ends just under the third's (folded)
        // header. The hole opening and closing as the drag moves shifts the
        // rows below it, so the target is judged frame by frame against the
        // rows as drawn -- and must settle on the third section, not the
        // second, over successive frames at the same pointer position.
        let mut idx = TrackIndex::default();
        idx.insert(ev(kind::API_SCOPE, 1, 1, 0, 0));
        idx.insert(ev(kind::VALUE, 1, 1, 0, 0));
        for depth in 0..8u8 {
            idx.insert(ev(kind::API_SCOPE, 2, 5, depth, 0));
        }
        idx.insert(ev(kind::VALUE, 2, 5, 0, 0));
        idx.insert(ev(kind::API_SCOPE, 3, 9, 0, 0));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.toggle(RowId::Process(3));
        strip.tick(1.0, &idx, None);
        let graph = LaneKey { pid: 1, tid: 1, kind: kind::VALUE, depth: 1, extra: 0 };
        let gy = strip.y[&RowId::Lane(graph)];
        let p3 = strip.y[&RowId::Process(3)];
        strip.begin_drag(TrackKey::Value(graph), gy, gy + 10.0);
        // Straight to just under process 3's header (as drawn at rest: the
        // graph leaving its section shifts everything below up by its
        // height, so aim from where the header will be drawn).
        let h = strip.block_h(TrackKey::Value(graph));
        let mut target = p3 - h + PROCESS_H + 6.0 + 10.0;
        for _ in 0..4 {
            strip.update_drag(target);
            strip.tick(0.0, &idx, None);
            // Keep the pointer under the header as it is drawn now.
            target = strip.y[&RowId::Process(3)] + PROCESS_H + 6.0 + 10.0;
        }
        strip.update_drag(target);
        strip.end_drag();
        assert_eq!(strip.host_pid(TrackKey::Value(graph)), 3, "order: {:?}", strip.track_order);
    }

    #[test]
    fn dropping_into_a_collapsed_section_lands_under_its_header() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1, 1));
        idx.insert(scope(1, 2, 2));
        idx.insert(scope(4, 7, 1));
        let mut strip = TrackStrip::default();
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        strip.toggle(RowId::Process(4));
        strip.tick(0.0, &idx, None);
        let t2 = ThreadId { pid: 1, tid: 2 };
        let y = strip.y[&RowId::Thread(t2)];
        strip.begin_drag(t2, y, y);
        strip.update_drag(strip.total_height() + 30.0);
        strip.tick(0.0, &idx, None);
        let hole = strip.insert_y().expect("a hole");
        let p4 = strip.y[&RowId::Process(4)];
        assert!((hole - (p4 + PROCESS_H)).abs() < 0.01, "hole {hole} right under the folded header at {p4}");
        strip.end_drag();
        assert_eq!(strip.host_pid(TrackKey::Thread(t2)), 4);
        // The drop unfolds the section: what was put there is seen there.
        assert!(!strip.collapsed(RowId::Process(4)));
        strip.tick(0.0, &idx, None);
        let ids = row_ids(&strip);
        let p4 = ids.iter().position(|i| *i == RowId::Process(4)).unwrap();
        assert_eq!(ids[p4 + 1], RowId::Thread(t2), "{ids:?}");
        // And a later sync does not fold it back.
        strip.sync(&idx, None);
        assert!(!strip.collapsed(RowId::Process(4)));
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
        let mid = strip.thread_order()[1];
        strip.toggle_hidden(mid);
        strip.sync(&idx, None);
        strip.tick(1.0, &idx, None);
        let first = strip.thread_order()[0];
        let last = strip.thread_order()[2];
        let y0 = strip.y.get(&RowId::Thread(first)).copied().unwrap_or(0.0);
        strip.begin_drag(first, y0, y0);
        strip.update_drag(y0 + 80.0);
        strip.end_drag();
        assert_eq!(strip.thread_order()[1], mid);
        assert_eq!(strip.thread_order()[0], last);
        assert_eq!(strip.thread_order()[2], first);
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
        let th = strip.thread_order()[0];
        assert_eq!(th, ThreadId { pid: 1, tid: 100 });
        assert!(!strip.thread_order().iter().any(|t| t.pid == 0 && t.tid == 0));
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
                .thread_order()
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
            strip.thread_order().is_empty(),
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
        let th = strip.thread_order()[0];
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
        assert!(strip.thread_order().contains(&th));
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
