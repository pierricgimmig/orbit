//! Vertical track-list inertia (phone flick / trackpad swipe).
//!
//! Offset is egui `ScrollArea` Y: 0 at the top, increasing down the stack.
//! A finger drag down decreases the offset so content follows 1:1. Releasing
//! with leftover velocity coasts; a new drag, click, WASD, or opposite flick
//! cancels or replaces the coast. The range is `[0, max]` — no bounce.

/// Exponential decay rate (1/s). Half-life ≈ 0.17 s (`ln(2) / 4`).
pub const DECAY_PER_SEC: f32 = 4.0;
/// Below this, the coast is treated as stopped (points / second).
pub const STOP_EPS: f32 = 30.0;
/// Release / swipe must exceed this to start a coast (points / second).
pub const FLICK_MIN: f32 = 180.0;
/// Consecutive wheel frames that count as a trackpad swipe, not a notch.
pub const WHEEL_STREAK_FOR_COAST: u8 = 3;
const SAMPLE_CAP: usize = 8;
const SAMPLE_WINDOW_S: f32 = 0.08;
const DT_MIN: f32 = 1.0 / 240.0;
const DT_MAX: f32 = 1.0 / 30.0;
const VEL_MAX: f32 = 8_000.0;

/// Flick / wheel coast for the lane list.
#[derive(Clone, Debug)]
pub struct VScrollInertia {
    velocity: f32,
    dragging: bool,
    samples: [(f32, f32); SAMPLE_CAP],
    n_samples: u8,
    sample_age: f32,
    wheel_streak: u8,
}

impl Default for VScrollInertia {
    fn default() -> Self {
        Self {
            velocity: 0.0,
            dragging: false,
            samples: [(0.0, 0.0); SAMPLE_CAP],
            n_samples: 0,
            sample_age: 0.0,
            wheel_streak: 0,
        }
    }
}

/// Inclusive scroll range: 0 at the top, `max` when the last track sits
/// on the bottom edge of the viewport.
#[inline]
pub fn clamp_offset(offset: f32, max: f32) -> f32 {
    offset.clamp(0.0, max.max(0.0))
}

/// How far the list can move. Zero when the content fits in the view.
#[inline]
pub fn max_offset(content_h: f32, view_h: f32) -> f32 {
    (content_h - view_h).max(0.0)
}

/// 1:1 finger mapping: dragging down reveals earlier tracks.
#[inline]
pub fn drag_offset(current: f32, drag_y: f32, max: f32) -> f32 {
    clamp_offset(current - drag_y, max)
}

impl VScrollInertia {
    #[inline]
    pub fn is_coasting(&self) -> bool {
        !self.dragging && self.velocity.abs() > STOP_EPS
    }

    #[inline]
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    #[inline]
    #[cfg(test)]
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    /// Drop velocity and samples. Finger-down, click, and WASD use this.
    pub fn cancel(&mut self) {
        self.velocity = 0.0;
        self.dragging = false;
        self.clear_samples();
        self.wheel_streak = 0;
    }

    /// Start a 1:1 drag; kills any leftover coast.
    pub fn begin_drag(&mut self) {
        self.cancel();
        self.dragging = true;
    }

    /// Follow the finger this frame. Does not coast.
    pub fn drag(&mut self, offset: f32, drag_y: f32, dt: f32, max: f32) -> f32 {
        if !self.dragging {
            self.begin_drag();
        }
        let next = drag_offset(offset, drag_y, max);
        self.record(dt, next - offset);
        next
    }

    /// Finger up: coast if the recent drag was a flick.
    pub fn end_drag(&mut self) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let v = self.filtered_velocity();
        self.clear_samples();
        self.velocity = if v.abs() >= FLICK_MIN {
            v.clamp(-VEL_MAX, VEL_MAX)
        } else {
            0.0
        };
    }

    /// Apply one wheel / trackpad Y delta (positive Y = toward the top).
    pub fn wheel(&mut self, offset: f32, scroll_y: f32, dt: f32, max: f32) -> f32 {
        self.dragging = false;
        let doffset = -scroll_y;
        if self.velocity * doffset < 0.0 {
            // Opposite swipe replaces the leftover coast immediately.
            self.velocity = 0.0;
            self.clear_samples();
            self.wheel_streak = 0;
        }
        let next = clamp_offset(offset + doffset, max);
        self.record(dt, next - offset);
        self.wheel_streak = self.wheel_streak.saturating_add(1);
        next
    }

    /// Wheel burst ended: keep velocity only for a fast multi-frame swipe.
    pub fn end_wheel_burst(&mut self) {
        if self.wheel_streak == 0 {
            return;
        }
        let v = self.filtered_velocity();
        self.velocity = if self.wheel_streak >= WHEEL_STREAK_FOR_COAST && v.abs() >= FLICK_MIN {
            v.clamp(-VEL_MAX, VEL_MAX)
        } else {
            0.0
        };
        self.wheel_streak = 0;
        self.clear_samples();
    }

    /// Integrate one coasting frame. Hits a bound → stop.
    pub fn tick(&mut self, offset: f32, dt: f32, max: f32) -> f32 {
        if self.dragging || !self.is_coasting() {
            if !self.dragging {
                self.velocity = 0.0;
            }
            return clamp_offset(offset, max);
        }
        let dt = dt.clamp(0.0, DT_MAX);
        let next = clamp_offset(offset + self.velocity * dt, max);
        if next <= 0.0 || next >= max.max(0.0) {
            self.velocity = 0.0;
            return next;
        }
        self.velocity *= (-DECAY_PER_SEC * dt).exp();
        if self.velocity.abs() <= STOP_EPS {
            self.velocity = 0.0;
        }
        next
    }

    fn record(&mut self, dt: f32, doffset: f32) {
        if !dt.is_finite() || dt <= 0.0 || !doffset.is_finite() {
            return;
        }
        let dt = dt.clamp(DT_MIN, DT_MAX);
        if self.n_samples as usize == SAMPLE_CAP {
            self.pop_oldest();
        }
        let i = self.n_samples as usize;
        self.samples[i] = (dt, doffset);
        self.n_samples += 1;
        self.sample_age += dt;
        while self.sample_age > SAMPLE_WINDOW_S && self.n_samples > 1 {
            self.pop_oldest();
        }
    }

    fn pop_oldest(&mut self) {
        if self.n_samples == 0 {
            return;
        }
        let (dt, _) = self.samples[0];
        self.sample_age = (self.sample_age - dt).max(0.0);
        for i in 1..self.n_samples as usize {
            self.samples[i - 1] = self.samples[i];
        }
        self.n_samples -= 1;
    }

    fn filtered_velocity(&self) -> f32 {
        let n = self.n_samples as usize;
        if n == 0 {
            return 0.0;
        }
        let mut t = 0.0;
        let mut d = 0.0;
        for i in 0..n {
            t += self.samples[i].0;
            d += self.samples[i].1;
        }
        if t <= 1e-4 {
            0.0
        } else {
            (d / t).clamp(-VEL_MAX, VEL_MAX)
        }
    }

    fn clear_samples(&mut self) {
        self.n_samples = 0;
        self.sample_age = 0.0;
    }
}

/// Velocity after `dt` of free decay (no bound).
#[cfg(test)]
pub fn decayed_velocity(velocity: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return velocity;
    }
    let v = velocity * (-DECAY_PER_SEC * dt).exp();
    if v.abs() <= STOP_EPS {
        0.0
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_offset_stays_in_track_list_bounds() {
        assert_eq!(clamp_offset(-10.0, 400.0), 0.0);
        assert_eq!(clamp_offset(200.0, 400.0), 200.0);
        assert_eq!(clamp_offset(500.0, 400.0), 400.0);
        assert_eq!(clamp_offset(10.0, 0.0), 0.0);
        assert_eq!(clamp_offset(10.0, -5.0), 0.0);
    }

    #[test]
    fn max_offset_is_zero_when_content_fits() {
        assert_eq!(max_offset(400.0, 500.0), 0.0);
        assert_eq!(max_offset(800.0, 500.0), 300.0);
    }

    #[test]
    fn drag_is_one_to_one_and_clamps() {
        assert_eq!(drag_offset(100.0, 30.0, 1000.0), 70.0);
        assert_eq!(drag_offset(100.0, -30.0, 1000.0), 130.0);
        assert_eq!(drag_offset(10.0, 40.0, 1000.0), 0.0);
        assert_eq!(drag_offset(990.0, -40.0, 1000.0), 1000.0);
    }

    #[test]
    fn decay_is_exponential_in_dt() {
        let v0 = 800.0;
        let dt = 0.05;
        let once = decayed_velocity(v0, dt);
        assert!((once - v0 * (-DECAY_PER_SEC * dt).exp()).abs() < 1e-4);
        let half = decayed_velocity(v0, dt * 0.5);
        let two_halves = decayed_velocity(half, dt * 0.5);
        assert!((two_halves - once).abs() < 1e-3);
    }

    #[test]
    fn tick_eases_then_stops() {
        let mut i = VScrollInertia::default();
        i.velocity = 400.0;
        let mut y = 0.0;
        let max = 10_000.0;
        for _ in 0..8 {
            y = i.tick(y, 1.0 / 60.0, max);
        }
        assert!(y > 0.0, "should have coasted, y={y}");
        assert!(i.velocity.abs() < 400.0);
        for _ in 0..180 {
            y = i.tick(y, 1.0 / 60.0, max);
        }
        assert!(!i.is_coasting());
        assert_eq!(i.velocity(), 0.0);
        assert!(y < max);
    }

    #[test]
    fn tick_clamps_and_cancels_at_bounds() {
        let mut i = VScrollInertia::default();
        i.velocity = -2_000.0;
        let y = i.tick(10.0, 1.0 / 30.0, 400.0);
        assert_eq!(y, 0.0);
        assert_eq!(i.velocity(), 0.0);
        assert!(!i.is_coasting());

        i.velocity = 2_000.0;
        let y = i.tick(390.0, 1.0 / 30.0, 400.0);
        assert_eq!(y, 400.0);
        assert_eq!(i.velocity(), 0.0);
    }

    #[test]
    fn cancel_zeros_velocity() {
        let mut i = VScrollInertia::default();
        i.velocity = 900.0;
        i.cancel();
        assert_eq!(i.velocity(), 0.0);
        assert!(!i.is_coasting());
        assert!(!i.is_dragging());
    }

    #[test]
    fn begin_drag_cancels_coast_and_follows_finger() {
        let mut i = VScrollInertia::default();
        i.velocity = 1_200.0;
        i.begin_drag();
        assert!(!i.is_coasting());
        let y = i.drag(200.0, 40.0, 1.0 / 60.0, 1_000.0);
        assert_eq!(y, 160.0);
        assert!(i.is_dragging());
        // Coast must not run while the finger is down.
        let still = i.tick(y, 1.0 / 60.0, 1_000.0);
        assert_eq!(still, 160.0);
    }

    #[test]
    fn end_drag_coasts_only_after_a_flick() {
        let mut flick = VScrollInertia::default();
        flick.begin_drag();
        let mut y = 400.0;
        for _ in 0..6 {
            y = flick.drag(y, 20.0, 1.0 / 60.0, 2_000.0);
        }
        flick.end_drag();
        assert!(
            flick.is_coasting(),
            "fast drag should coast, v={}",
            flick.velocity()
        );
        assert!(flick.velocity() < 0.0, "finger down → offset decreases");

        let mut tap = VScrollInertia::default();
        tap.begin_drag();
        let y = tap.drag(400.0, 2.0, 1.0 / 60.0, 2_000.0);
        tap.end_drag();
        assert!(!tap.is_coasting(), "slow drag must not coast, y={y}");
    }

    #[test]
    fn wheel_single_notch_does_not_coast() {
        let mut i = VScrollInertia::default();
        let y = i.wheel(200.0, 40.0, 1.0 / 60.0, 1_000.0);
        assert_eq!(y, 160.0);
        i.end_wheel_burst();
        assert!(!i.is_coasting());
    }

    #[test]
    fn wheel_swipe_coasts_then_opposite_replaces() {
        let mut i = VScrollInertia::default();
        let mut y = 400.0;
        for _ in 0..5 {
            y = i.wheel(y, 24.0, 1.0 / 60.0, 2_000.0);
        }
        i.end_wheel_burst();
        assert!(i.is_coasting(), "v={}", i.velocity());
        assert!(i.velocity() < 0.0);

        // Opposite burst replaces.
        y = i.wheel(y, -24.0, 1.0 / 60.0, 2_000.0);
        for _ in 0..4 {
            y = i.wheel(y, -24.0, 1.0 / 60.0, 2_000.0);
        }
        i.end_wheel_burst();
        assert!(i.is_coasting());
        assert!(
            i.velocity() > 0.0,
            "opposite swipe should coast down, y={y}"
        );
    }
}
