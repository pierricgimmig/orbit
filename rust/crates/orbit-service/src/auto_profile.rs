// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Auto-profiling: the service picks what to hook on its own.
//!
//! Every step (a couple of seconds of capture) it looks at two things: the
//! sampling report of the last few seconds, which says where the time goes,
//! and how many calls each hooked function made since the last step, which
//! says what hooking it costs. From those it unhooks what does not pay --
//! a function too hot to time with a hook (the samples already show it), one
//! that never returned in the window (a main loop entered before it was
//! hooked, a path the program left), and the hottest of the rest while the
//! total is over budget -- and hooks the next functions the samples point at,
//! highest inclusive share first, while there is room under the budget and
//! the hook cap. It converges on a set that gives an overview of where the
//! time goes at a cost the target does not feel, and keeps going: a function
//! that went quiet is tried again later, so a new code path gets picked up.
//!
//! Pure: the capture loop feeds it the report and the counts and carries out
//! what it returns, so the rules are tested without a kernel.

use std::collections::{BTreeMap, HashMap};

/// The knobs, from the settings and the hook cap.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Scopes a second, across every hooked function, the set aims to stay
    /// under.
    pub budget_per_s: f64,
    /// The engine's hook cap.
    pub max_hooks: usize,
    /// At most this many new hooks a step, so the cost of each can be seen
    /// before the next ones go in.
    pub add_per_step: usize,
    /// A function under this share of the samples (inclusive) is noise, not
    /// an overview.
    pub min_inclusive_percent: f32,
    /// Steps a function that went quiet waits before it is tried again.
    pub retry_after_steps: u32,
}

impl Config {
    pub fn with_budget(budget_per_s: u64, max_hooks: usize) -> Config {
        Config {
            budget_per_s: budget_per_s.max(1) as f64,
            max_hooks,
            add_per_step: 3,
            min_inclusive_percent: 1.0,
            retry_after_steps: 15,
        }
    }

    /// One function past this is too hot to keep: half the budget on its own.
    pub fn hot_per_s(&self) -> f64 {
        self.budget_per_s / 2.0
    }
}

/// A function the sampling report offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub function_id: u64,
    pub name: String,
    pub inclusive_percent: f32,
    /// Resolvable to a file offset and not judged unsafe (or risky) to hook.
    pub hookable: bool,
}

/// Why a function was hooked or unhooked; also what the timeline says.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    /// It holds this share of the samples.
    Sampled { inclusive_percent: f32 },
    /// It fired this often on its own.
    TooHot { per_s: f64 },
    /// It never completed a call in the last step.
    Silent,
    /// The set was over budget and this was its hottest member.
    OverBudget { per_s: f64 },
    /// The call-rate limit switched it off already.
    Limited,
}

impl Why {
    pub fn text(&self) -> String {
        match self {
            Why::Sampled { inclusive_percent } => format!("{inclusive_percent:.1}% of samples"),
            Why::TooHot { per_s } => format!("too hot, {per_s:.0} calls/s"),
            Why::Silent => "no calls".to_string(),
            Why::OverBudget { per_s } => format!("over budget, {per_s:.0} calls/s"),
            Why::Limited => "call-rate limit".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Hook { function_id: u64, name: String, why: Why },
    Unhook { function_id: u64, name: String, why: Why },
}

#[derive(Clone, Debug)]
struct Hooked {
    name: String,
    /// The step it was hooked at: it is judged from the next one on.
    since_step: u32,
    /// Completed calls and seconds watched since it was hooked, halved
    /// together past `MEMORY_S` so the rate follows what the program does
    /// now. Accumulated rather than per step, so a step cut short by a hot
    /// hook does not judge a quiet one on a few milliseconds.
    calls: f64,
    watched_s: f64,
    /// `calls / watched_s`.
    per_s: f64,
}

/// Seconds of history a hook's rate remembers, roughly.
const MEMORY_S: f64 = 10.0;
/// A hook watched this long with no call is silent.
const SILENT_AFTER_S: f64 = 1.5;
/// Calls before a rate is trusted enough to call a hook too hot.
const HOT_MIN_CALLS: f64 = 10.0;

/// Where the controller stands, for the status line and `/api/status`.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub step: u32,
    pub budget_per_s: f64,
    /// Scopes a second the hooked set produced over the last step.
    pub total_per_s: f64,
    /// (name, calls a second) of every hooked function, busiest first.
    pub hooked: Vec<(String, f64)>,
    pub converged: bool,
    /// Every function it has taken off so far.
    pub unhooked: u32,
}

pub struct AutoProfiler {
    config: Config,
    step: u32,
    hooked: BTreeMap<u64, Hooked>,
    /// Function -> the step it may be tried again at (`u32::MAX`: never).
    rejected: HashMap<u64, u32>,
    /// Steps in a row that changed nothing.
    quiet_steps: u32,
    total_per_s: f64,
    unhooked: u32,
}

impl AutoProfiler {
    pub fn new(config: Config) -> AutoProfiler {
        AutoProfiler {
            config,
            step: 0,
            hooked: BTreeMap::new(),
            rejected: HashMap::new(),
            quiet_steps: 0,
            total_per_s: 0.0,
            unhooked: 0,
        }
    }

    /// The budget changed (the setting was edited mid-capture).
    pub fn set_budget(&mut self, budget_per_s: u64) {
        self.config.budget_per_s = budget_per_s.max(1) as f64;
    }

    /// Calls past which one hook is too hot, a second.
    pub fn hot_per_s(&self) -> f64 {
        self.config.hot_per_s()
    }

    /// The controller hooked this function (and has not unhooked it).
    pub fn is_hooked(&self, function_id: u64) -> bool {
        self.hooked.contains_key(&function_id)
    }

    /// A hook the controller asked for did not arm: forget it, and never
    /// ask again this capture.
    pub fn hook_failed(&mut self, function_id: u64) {
        self.hooked.remove(&function_id);
        self.rejected.insert(function_id, u32::MAX);
    }

    /// One step. `counts` is completed calls per hooked function since the
    /// last step, `window_s` how long that was; `limited` the functions the
    /// call-rate limit has switched off; `candidates` the sampling report of
    /// the last few seconds.
    pub fn step(
        &mut self,
        candidates: &[Candidate],
        counts: &HashMap<u64, u64>,
        window_s: f64,
        limited: &[u64],
    ) -> Vec<Action> {
        self.step += 1;
        let step = self.step;
        self.rejected.retain(|_, until| *until > step);
        let mut actions = Vec::new();
        let hot = self.config.hot_per_s();
        let window_s = window_s.max(1e-3);
        let retry = self.config.retry_after_steps;

        // What each hook has cost since it went in.
        for (id, h) in self.hooked.iter_mut() {
            if h.since_step < step {
                h.calls += counts.get(id).copied().unwrap_or(0) as f64;
                h.watched_s += window_s;
                if h.watched_s > MEMORY_S {
                    h.calls /= 2.0;
                    h.watched_s /= 2.0;
                }
                h.per_s = h.calls / h.watched_s;
            }
        }
        let mut unhook = |this: &mut Self, id: u64, why: Why, retry: Option<u32>| {
            if let Some(h) = this.hooked.remove(&id) {
                this.rejected.insert(id, retry.map_or(u32::MAX, |r| step.saturating_add(r)));
                this.unhooked += 1;
                actions.push(Action::Unhook { function_id: id, name: h.name, why });
            }
        };
        for id in limited {
            unhook(self, *id, Why::Limited, None);
        }
        let judged: Vec<(u64, f64, f64, f64)> = self
            .hooked
            .iter()
            .filter(|(_, h)| h.since_step < step)
            .map(|(id, h)| (*id, h.per_s, h.calls, h.watched_s))
            .collect();
        for (id, per_s, calls, watched_s) in &judged {
            if *per_s > hot && *calls >= HOT_MIN_CALLS {
                unhook(self, *id, Why::TooHot { per_s: *per_s }, None);
            } else if *calls == 0.0 && *watched_s >= SILENT_AFTER_S {
                unhook(self, *id, Why::Silent, Some(retry));
            }
        }
        // Still over: the hottest go first, until the rest fits.
        let mut total: f64 = self.hooked.values().map(|h| h.per_s).sum();
        while total > self.config.budget_per_s {
            let Some((id, per_s)) = self
                .hooked
                .iter()
                .max_by(|a, b| a.1.per_s.total_cmp(&b.1.per_s))
                .map(|(id, h)| (*id, h.per_s))
            else {
                break;
            };
            unhook(self, id, Why::OverBudget { per_s }, Some(retry * 2));
            total -= per_s;
        }
        self.total_per_s = total;

        // Room under the budget and the cap: the next functions the samples
        // point at, by inclusive share. A quarter of the budget is kept free
        // for what the new ones will cost.
        let room = self.config.max_hooks.saturating_sub(self.hooked.len());
        if room > 0 && total < self.config.budget_per_s * 0.75 {
            let mut offered: Vec<&Candidate> = candidates
                .iter()
                .filter(|c| {
                    c.hookable
                        && c.function_id != 0
                        && c.inclusive_percent >= self.config.min_inclusive_percent
                        && !self.hooked.contains_key(&c.function_id)
                        && !self.rejected.contains_key(&c.function_id)
                })
                .collect();
            offered.sort_by(|a, b| b.inclusive_percent.total_cmp(&a.inclusive_percent));
            for c in offered.into_iter().take(room.min(self.config.add_per_step)) {
                self.hooked.insert(
                    c.function_id,
                    Hooked { name: c.name.clone(), since_step: step, calls: 0.0, watched_s: 0.0, per_s: 0.0 },
                );
                actions.push(Action::Hook {
                    function_id: c.function_id,
                    name: c.name.clone(),
                    why: Why::Sampled { inclusive_percent: c.inclusive_percent },
                });
            }
        }
        self.quiet_steps = if actions.is_empty() { self.quiet_steps + 1 } else { 0 };
        actions
    }

    pub fn status(&self) -> Status {
        let mut hooked: Vec<(String, f64)> = self.hooked.values().map(|h| (h.name.clone(), h.per_s)).collect();
        hooked.sort_by(|a, b| b.1.total_cmp(&a.1));
        Status {
            step: self.step,
            budget_per_s: self.config.budget_per_s,
            total_per_s: self.total_per_s,
            hooked,
            converged: !self.hooked.is_empty() && self.quiet_steps >= 2,
            unhooked: self.unhooked,
        }
    }
}

/// The candidates in a sampling report (`SampleStore::report_json_for_ranges`),
/// with `hookable` from the caller's knowledge of each function.
pub fn candidates_from_report(report_json: &str, hookable: impl Fn(u64) -> bool) -> Vec<Candidate> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(report_json) else { return Vec::new() };
    let Some(rows) = value.get("functions").and_then(|r| r.as_array()) else { return Vec::new() };
    rows.iter()
        .filter_map(|row| {
            let function_id = row.get("function_id")?.as_u64()?;
            Some(Candidate {
                function_id,
                name: row.get("name")?.as_str()?.to_string(),
                inclusive_percent: row.get("inclusive_percent")?.as_f64()? as f32,
                hookable: function_id != 0 && hookable(function_id),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: u64, pct: f32) -> Candidate {
        Candidate { function_id: id, name: format!("f{id}"), inclusive_percent: pct, hookable: true }
    }

    fn hooks(actions: &[Action]) -> Vec<u64> {
        actions.iter().filter_map(|a| match a { Action::Hook { function_id, .. } => Some(*function_id), _ => None }).collect()
    }

    fn unhooks(actions: &[Action]) -> Vec<(u64, Why)> {
        actions
            .iter()
            .filter_map(|a| match a { Action::Unhook { function_id, why, .. } => Some((*function_id, why.clone())), _ => None })
            .collect()
    }

    fn profiler() -> AutoProfiler {
        AutoProfiler::new(Config::with_budget(1000, 16))
    }

    #[test]
    fn the_first_step_hooks_the_widest_functions_a_few_at_a_time() {
        let mut p = profiler();
        let c = [cand(1, 10.0), cand(2, 90.0), cand(3, 50.0), cand(4, 30.0), cand(5, 0.5)];
        let a = p.step(&c, &HashMap::new(), 2.0, &[]);
        assert_eq!(hooks(&a), vec![2, 3, 4]);
        // Next step: the rest that clear the noise floor, not the 0.5% one.
        let counts = HashMap::from([(2, 20), (3, 20), (4, 20)]);
        assert_eq!(hooks(&p.step(&c, &counts, 2.0, &[])), vec![1]);
    }

    #[test]
    fn unsafe_and_unresolvable_functions_are_never_offered() {
        let mut p = profiler();
        let mut risky = cand(1, 80.0);
        risky.hookable = false;
        let a = p.step(&[risky, cand(0, 70.0), cand(2, 10.0)], &HashMap::new(), 2.0, &[]);
        assert_eq!(hooks(&a), vec![2]);
    }

    #[test]
    fn a_hot_function_is_unhooked_and_not_tried_again() {
        let mut p = profiler();
        let c = [cand(1, 60.0), cand(2, 40.0)];
        p.step(&c, &HashMap::new(), 2.0, &[]);
        // 1 fired 3000 calls in 2 s: 1500/s, past half the budget.
        let a = p.step(&c, &HashMap::from([(1, 3000), (2, 100)]), 2.0, &[]);
        assert_eq!(unhooks(&a), vec![(1, Why::TooHot { per_s: 1500.0 })]);
        for _ in 0..40 {
            let a = p.step(&c, &HashMap::from([(2, 100)]), 2.0, &[]);
            assert!(!hooks(&a).contains(&1), "a too-hot function stays off");
        }
    }

    #[test]
    fn a_silent_function_is_unhooked_and_tried_again_later() {
        let mut p = AutoProfiler::new(Config { retry_after_steps: 3, ..Config::with_budget(1000, 16) });
        let c = [cand(1, 60.0)];
        p.step(&c, &HashMap::new(), 2.0, &[]);
        let a = p.step(&c, &HashMap::new(), 2.0, &[]);
        assert_eq!(unhooks(&a), vec![(1, Why::Silent)]);
        assert!(hooks(&p.step(&c, &HashMap::new(), 2.0, &[])).is_empty());
        assert!(hooks(&p.step(&c, &HashMap::new(), 2.0, &[])).is_empty());
        assert_eq!(hooks(&p.step(&c, &HashMap::new(), 2.0, &[])), vec![1], "retried after the wait");
    }

    #[test]
    fn a_new_hook_is_not_judged_before_a_full_window() {
        let mut p = profiler();
        let a = p.step(&[cand(1, 60.0)], &HashMap::new(), 2.0, &[]);
        assert_eq!(hooks(&a), vec![1]);
        // Hooked this step: no calls counted yet, and that is not silence.
        assert!(unhooks(&a).is_empty());
    }

    #[test]
    fn a_short_step_prunes_a_hot_hook_but_does_not_call_a_quiet_one_silent() {
        let mut p = profiler();
        let c = [cand(1, 60.0), cand(2, 40.0)];
        p.step(&c, &HashMap::new(), 2.0, &[]);
        // Cut short after 20 ms because 1 already made 1000 calls; 2, a
        // once-a-frame function, made none yet.
        let a = p.step(&c, &HashMap::from([(1, 1000)]), 0.02, &[]);
        assert_eq!(unhooks(&a), vec![(1, Why::TooHot { per_s: 50_000.0 })]);
        assert!(p.is_hooked(2), "20 ms without a call is not silence");
        // A full second and a half with none is.
        let a = p.step(&c, &HashMap::new(), 1.5, &[]);
        assert_eq!(unhooks(&a), vec![(2, Why::Silent)]);
    }

    #[test]
    fn over_budget_the_hottest_go_until_the_rest_fits() {
        let mut p = profiler();
        let c: Vec<Candidate> = (1..=3).map(|i| cand(i, 50.0 - i as f32)).collect();
        p.step(&c, &HashMap::new(), 1.0, &[]);
        // 450 + 400 + 300 = 1150/s: each under the hot limit, together over.
        let a = p.step(&c, &HashMap::from([(1, 450), (2, 400), (3, 300)]), 1.0, &[]);
        assert_eq!(unhooks(&a), vec![(1, Why::OverBudget { per_s: 450.0 })]);
        assert_eq!(p.status().total_per_s, 700.0);
    }

    #[test]
    fn nothing_is_added_past_the_cap_or_near_the_budget() {
        let mut p = AutoProfiler::new(Config::with_budget(1000, 2));
        let c: Vec<Candidate> = (1..=5).map(|i| cand(i, 50.0)).collect();
        assert_eq!(hooks(&p.step(&c, &HashMap::new(), 1.0, &[])).len(), 2);
        assert!(hooks(&p.step(&c, &HashMap::from([(1, 10), (2, 10)]), 1.0, &[])).is_empty(), "at the cap");
        let mut p = profiler();
        p.step(&c[..1], &HashMap::new(), 1.0, &[]);
        // 1 alone makes 480/s, under the hot limit; 480 < 750 leaves room.
        assert_eq!(hooks(&p.step(&c, &HashMap::from([(1, 480)]), 1.0, &[])).len(), 3);
        // Now 480 + 3 x 100 = 780/s: past three quarters, nothing more.
        let counts = HashMap::from([(1, 480), (2, 100), (3, 100), (4, 100)]);
        assert!(hooks(&p.step(&c, &counts, 1.0, &[])).is_empty());
    }

    #[test]
    fn it_converges_when_steps_stop_changing_anything() {
        let mut p = profiler();
        let c = [cand(1, 60.0), cand(2, 40.0)];
        p.step(&c, &HashMap::new(), 2.0, &[]);
        let counts = HashMap::from([(1, 120), (2, 120)]);
        assert!(!p.status().converged);
        p.step(&c, &counts, 2.0, &[]);
        p.step(&c, &counts, 2.0, &[]);
        let s = p.status();
        assert!(s.converged);
        assert_eq!(s.total_per_s, 120.0);
        assert_eq!(s.hooked.len(), 2);
    }

    #[test]
    fn a_function_the_call_limit_switched_off_leaves_the_set() {
        let mut p = profiler();
        p.step(&[cand(1, 60.0)], &HashMap::new(), 2.0, &[]);
        let a = p.step(&[cand(1, 60.0)], &HashMap::new(), 2.0, &[1]);
        assert_eq!(unhooks(&a), vec![(1, Why::Limited)]);
        assert!(p.status().hooked.is_empty());
    }

    #[test]
    fn a_failed_hook_is_forgotten_and_not_asked_for_again() {
        let mut p = profiler();
        p.step(&[cand(1, 60.0)], &HashMap::new(), 2.0, &[]);
        p.hook_failed(1);
        assert!(p.status().hooked.is_empty());
        assert!(hooks(&p.step(&[cand(1, 60.0)], &HashMap::new(), 2.0, &[])).is_empty());
    }

    #[test]
    fn candidates_are_read_from_a_sampling_report() {
        let json = r#"{"samples":10,"functions":[
            {"name":"tick","module":"a","function_id":7,"self":1,"inclusive":9,"self_percent":10.0,"inclusive_percent":90.0},
            {"name":"[unknown]","module":"","function_id":0,"self":1,"inclusive":1,"self_percent":10.0,"inclusive_percent":10.0}]}"#;
        let c = candidates_from_report(json, |id| id != 99);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], Candidate { function_id: 7, name: "tick".into(), inclusive_percent: 90.0, hookable: true });
        assert!(!c[1].hookable, "no function id, nothing to hook");
        assert!(candidates_from_report("not json", |_| true).is_empty());
    }
}
