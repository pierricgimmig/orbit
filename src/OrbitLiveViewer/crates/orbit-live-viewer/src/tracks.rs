//! Session track order and damped reorder. Lanes, not scopes.

use std::collections::HashMap;

use orbit_live_event::LaneKey;
use orbit_live_render::{
    drop_index_for_y, lane_gap, lane_height, reorder_insert, stacked_layout, sync_lane_order,
    TrackIndex,
};

pub struct TrackStrip {
    pub order: Vec<LaneKey>,
    y: HashMap<LaneKey, f32>,
    drag: Option<Drag>,
}

struct Drag {
    key: LaneKey,
    grab_off: f32,
    pointer_y: f32,
}

impl Default for TrackStrip {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            y: HashMap::new(),
            drag: None,
        }
    }
}

impl TrackStrip {
    pub fn sync(&mut self, index: &TrackIndex) {
        sync_lane_order(&mut self.order, index);
        for &k in &self.order {
            self.y.entry(k).or_insert(0.0);
        }
        self.y.retain(|k, _| self.order.iter().any(|o| o == k));
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn is_dragging(&self, key: LaneKey) -> bool {
        self.drag.as_ref().map(|d| d.key == key).unwrap_or(false)
    }

    pub fn preview_order(&self) -> Vec<LaneKey> {
        let Some(d) = &self.drag else {
            return self.order.clone();
        };
        let dest = drop_index_for_y(&self.order, d.key, d.pointer_y - d.grab_off + 0.5);
        reorder_insert(&self.order, d.key, dest)
    }

    pub fn tick(&mut self, dt: f32) {
        let preview = self.preview_order();
        let targets = stacked_layout(&preview, 0.0);
        let k = 1.0 - (-dt / 0.08).exp();
        for (key, target) in targets {
            let y = self.y.entry(key).or_insert(target);
            *y += (target - *y) * k;
        }
        if let Some(d) = &self.drag {
            self.y.insert(d.key, d.pointer_y - d.grab_off);
        }
    }

    pub fn layout(&self) -> Vec<(LaneKey, f32)> {
        self.preview_order()
            .into_iter()
            .map(|k| (k, *self.y.get(&k).unwrap_or(&0.0)))
            .collect()
    }

    pub fn begin_drag(&mut self, key: LaneKey, lane_y: f32, pointer_y: f32) {
        self.drag = Some(Drag {
            key,
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
            self.order = self.preview_order();
        }
        self.drag = None;
    }

    pub fn total_height(&self) -> f32 {
        self.order
            .iter()
            .map(|k| lane_height(*k) + lane_gap(*k))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{kind, LiveEvent};

    fn scope(tid: u32, name: u32) -> LiveEvent {
        LiveEvent {
            start_ns: 0,
            duration_ns: 10,
            tid,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: name,
        }
    }

    #[test]
    fn drag_end_persists_session_order() {
        let mut idx = TrackIndex::default();
        idx.insert(scope(1, 1));
        idx.insert(scope(2, 2));
        let mut strip = TrackStrip::default();
        strip.sync(&idx);
        assert_eq!(strip.order.len(), 2);
        let first = strip.order[0];
        let second = strip.order[1];
        strip.begin_drag(first, 0.0, 0.0);
        strip.update_drag(40.0);
        strip.end_drag();
        assert_eq!(strip.order[0], second);
        assert_eq!(strip.order[1], first);
    }
}
