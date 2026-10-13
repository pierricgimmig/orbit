// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Ask panel. Questions go to orbit-service; answers come back with
//! `orbit://` citations the timeline can open.

use eframe::egui::{Button, Color32, Frame, Label, Margin, RichText, ScrollArea, TextEdit, Ui, Vec2};
use orbit_live_insight::{
    message_segments, AiStatus, ChatResponse, EvidenceTarget, Intent, Segment, Turn, UiAction,
};

use crate::fonts;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    Proposed,
    Armed,
    Dismissed,
}

#[derive(Clone, Debug)]
pub struct HookCard {
    pub name: String,
    pub function_id: u64,
    pub module: String,
    pub reason: String,
    pub state: HookState,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Panel {
    pub lines: Vec<Line>,
    pub input: String,
    pub pending: bool,
    pub error: String,
    pub status: Option<AiStatus>,
    pub status_at: f64,
    pub hooks: Vec<HookCard>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            input: String::new(),
            pending: false,
            error: String::new(),
            status: None,
            status_at: -10.0,
            hooks: Vec::new(),
        }
    }
}

impl Panel {
    pub fn enabled(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.enabled)
    }

    pub fn history(&self) -> Vec<Turn> {
        self.lines
            .iter()
            .filter(|l| l.role == "user" || l.role == "assistant")
            .map(|l| Turn { role: l.role.clone(), content: l.text.clone() })
            .collect()
    }

    pub fn push_user(&mut self, text: String) {
        self.lines.push(Line { role: "user".into(), text });
        self.error.clear();
    }

    pub fn push_reply(&mut self, response: &ChatResponse) {
        if !response.message.trim().is_empty() {
            self.lines.push(Line { role: "assistant".into(), text: response.message.clone() });
        }
        for action in &response.actions {
            if let UiAction::SuggestHook { name, function_id, module, reason } = action {
                if self.hooks.iter().any(|h| h.name == *name && h.function_id == *function_id && h.state == HookState::Proposed)
                {
                    continue;
                }
                self.hooks.push(HookCard {
                    name: name.clone(),
                    function_id: *function_id,
                    module: module.clone(),
                    reason: reason.clone(),
                    state: HookState::Proposed,
                });
            }
        }
    }

    pub fn note(&mut self, text: impl Into<String>) {
        self.lines.push(Line { role: "note".into(), text: text.into() });
    }
}

pub enum Event {
    Send { intent: Intent, text: String },
    Activate(EvidenceTarget),
    ConfirmHook(usize),
    DismissHook(usize),
}

/// Draw the conversation. `narrow` is the phone layout: stacked controls and
/// larger tap targets.
pub fn draw(ui: &mut Ui, panel: &mut Panel, narrow: bool) -> Vec<Event> {
    let mut events = Vec::new();
    let enabled = panel.enabled();
    let button_h = if narrow { 36.0 } else { 24.0 };

    ui.label(
        RichText::new("Ask about this capture")
            .family(fonts::medium())
            .size(if narrow { 15.0 } else { 13.0 })
            .color(theme::TEXT()),
    );
    ui.label(
        RichText::new("Answers use the samples, scopes, thread states and the current selection. Claims link back into the trace.")
            .size(11.0)
            .color(theme::MUTED()),
    );
    ui.add_space(4.0);

    match &panel.status {
        None => {
            ui.label(RichText::new("Checking whether a model is configured…").size(11.0).color(theme::MUTED()));
        }
        Some(status) if !status.enabled => {
            Frame::new()
                .fill(theme::INPUT())
                .inner_margin(Margin::symmetric(10, 8))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Assistant off").family(fonts::medium()).size(12.0).color(theme::TEXT()));
                    ui.add_space(4.0);
                    ui.add(Label::new(RichText::new(&status.hint).size(11.5).color(theme::TEXT())).wrap());
                });
            ui.add_space(6.0);
            ui.label(RichText::new(&status.privacy).size(10.5).color(theme::MUTED()));
        }
        Some(status) => {
            let who = if status.model.is_empty() {
                status.provider.clone()
            } else {
                format!("{} · {}", status.provider, status.model)
            };
            ui.label(RichText::new(who).size(10.5).color(theme::MUTED()));
            ui.label(RichText::new(&status.privacy).size(10.5).color(theme::MUTED()));
        }
    }

    if !panel.error.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(&panel.error).size(11.5).color(Color32::from_rgb(0xE0, 0x80, 0x70)));
    }

    ui.add_space(6.0);
    prompt_row(ui, panel, narrow, button_h, enabled, &mut events);

    let footer = if narrow { 92.0 } else { 72.0 };
    let scroll_h = (ui.available_height() - footer).max(if narrow { 96.0 } else { 80.0 });
    ScrollArea::vertical()
        .max_height(scroll_h)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if panel.lines.is_empty() && panel.hooks.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Ask why a frame is slow, or what to optimize. Explain and Suggest use the current selection, or the whole capture when nothing is selected.")
                        .size(11.5)
                        .color(theme::MUTED()),
                );
            }
            for line in &panel.lines {
                ui.add_space(6.0);
                draw_line(ui, line, &mut events);
            }
            for (i, hook) in panel.hooks.iter().enumerate() {
                if hook.state == HookState::Dismissed {
                    continue;
                }
                ui.add_space(6.0);
                draw_hook(ui, i, hook, narrow, button_h, &mut events);
            }
            if panel.pending {
                ui.add_space(8.0);
                ui.label(RichText::new("Looking at the capture…").size(11.5).color(theme::ACCENT()));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let send_w = if narrow { 72.0 } else { 56.0 };
        let edit = ui.add_enabled(
            enabled && !panel.pending,
            TextEdit::singleline(&mut panel.input)
                .desired_width((ui.available_width() - send_w - 8.0).max(40.0))
                .hint_text("Why is this slow?")
                .font(eframe::egui::FontId::proportional(if narrow { 14.0 } else { 13.0 }))
                .margin(Margin::symmetric(8, if narrow { 8 } else { 4 }))
                .background_color(theme::INPUT()),
        );
        let submit = edit.lost_focus() && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter));
        let send = ui.add_enabled(
            enabled && !panel.pending && !panel.input.trim().is_empty(),
            Button::new(RichText::new("Send").size(if narrow { 14.0 } else { 12.0 })).min_size(Vec2::new(send_w, button_h)),
        );
        crate::app::note_ui_rect("ask:send", send.rect);
        if (send.clicked() || submit) && enabled && !panel.pending {
            let text = panel.input.trim().to_string();
            if !text.is_empty() {
                panel.input.clear();
                events.push(Event::Send { intent: Intent::Question, text });
            }
        }
    });
    events
}

fn prompt_row(ui: &mut Ui, panel: &Panel, narrow: bool, button_h: f32, enabled: bool, events: &mut Vec<Event>) {
    let explain = "Explain this selection";
    let suggest = "Suggest optimizations";
    let mut go = |ui: &mut Ui, label: &str, intent: Intent, prompt: &str| {
        let resp = ui.add_enabled(
            enabled && !panel.pending,
            Button::new(RichText::new(label).size(if narrow { 13.0 } else { 11.5 }).color(theme::TEXT()))
                .fill(theme::TRACK())
                .min_size(Vec2::new(if narrow { ui.available_width() } else { 0.0 }, button_h)),
        );
        crate::app::note_ui_rect(
            match intent {
                Intent::ExplainSelection => "ask:explain",
                Intent::Suggest => "ask:suggest",
                Intent::Question => "ask:question",
            },
            resp.rect,
        );
        if resp.clicked() {
            events.push(Event::Send { intent, text: prompt.into() });
        }
    };
    if narrow {
        go(ui, explain, Intent::ExplainSelection, EXPLAIN);
        go(ui, suggest, Intent::Suggest, SUGGEST);
    } else {
        ui.horizontal_wrapped(|ui| {
            go(ui, explain, Intent::ExplainSelection, EXPLAIN);
            go(ui, suggest, Intent::Suggest, SUGGEST);
        });
    }
}

const EXPLAIN: &str = "Explain this selection. What is slow, and which functions account for it?";
const SUGGEST: &str = "What should I optimize in this selection? Suggest the highest-value changes.";

fn draw_line(ui: &mut Ui, line: &Line, events: &mut Vec<Event>) {
    let (fill, who) = match line.role.as_str() {
        "user" => (theme::TRACK(), "You"),
        "note" => (theme::PANEL(), ""),
        _ => (theme::RAIL(), "Assistant"),
    };
    Frame::new()
        .fill(fill)
        .inner_margin(Margin::symmetric(8, 6))
        .corner_radius(6.0)
        .show(ui, |ui| {
            if !who.is_empty() {
                ui.label(RichText::new(who).size(10.0).color(theme::MUTED()));
            }
            for (i, row) in line.text.split('\n').enumerate() {
                if i > 0 && row.is_empty() {
                    ui.add_space(4.0);
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for seg in message_segments(row) {
                        match seg {
                            Segment::Text(text) => {
                                if !text.is_empty() {
                                    ui.add(Label::new(RichText::new(text).size(12.5).color(theme::TEXT())).wrap());
                                }
                            }
                            Segment::Link { label, target } => {
                                let resp = ui.add(
                                    Button::new(RichText::new(label).size(12.0).color(theme::ACCENT())).fill(theme::INPUT()),
                                );
                                if resp.clicked() {
                                    events.push(Event::Activate(target));
                                }
                            }
                        }
                    }
                });
            }
        });
}

fn draw_hook(ui: &mut Ui, index: usize, hook: &HookCard, narrow: bool, button_h: f32, events: &mut Vec<Event>) {
    Frame::new()
        .fill(theme::INPUT())
        .inner_margin(Margin::symmetric(8, 6))
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.label(RichText::new(format!("Hook {}", hook.name)).family(fonts::medium()).size(12.0).color(theme::TEXT()));
            if !hook.reason.is_empty() {
                ui.add(Label::new(RichText::new(&hook.reason).size(11.5).color(theme::TEXT())).wrap());
            }
            ui.label(
                RichText::new("Nothing is hooked until you confirm. The next Record arms it.")
                    .size(10.5)
                    .color(theme::MUTED()),
            );
            match hook.state {
                HookState::Armed => {
                    ui.label(RichText::new("Hooked for the next recording.").size(11.5).color(theme::ACCENT()));
                }
                HookState::Dismissed => {}
                HookState::Proposed => {
                    let mut row = |ui: &mut Ui| {
                        let hook_btn = ui.add(
                            Button::new(RichText::new("Hook").size(12.0).color(theme::CANVAS()))
                                .fill(theme::ACCENT())
                                .min_size(Vec2::new(if narrow { 96.0 } else { 64.0 }, button_h)),
                        );
                        crate::app::note_ui_rect("ask:hook", hook_btn.rect);
                        if hook_btn.clicked() {
                            events.push(Event::ConfirmHook(index));
                        }
                        let no = ui.add(
                            Button::new(RichText::new("Not now").size(12.0))
                                .min_size(Vec2::new(if narrow { 96.0 } else { 72.0 }, button_h)),
                        );
                        if no.clicked() {
                            events.push(Event::DismissHook(index));
                        }
                    };
                    if narrow {
                        ui.horizontal(|ui| row(ui));
                    } else {
                        ui.horizontal(|ui| row(ui));
                    }
                }
            }
        });
}
