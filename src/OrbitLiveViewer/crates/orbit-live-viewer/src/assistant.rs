// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The assistant panel.
//!
//! Questions go to orbit-service, which calls the model. This module draws
//! the answer, the evidence chips, and the hook confirmation. It does not
//! hold an API key. The action types are the shared `orbit-assistant` crate
//! so the panel and the service cannot drift apart.

use orbit_assistant::{Evidence, UiAction};
use serde::Deserialize;

#[cfg(feature = "egui")]
use eframe::egui::{
    self, Align, Color32, FontId, Frame, Layout, Margin, RichText, ScrollArea, Stroke, TextEdit,
    Ui, Vec2,
};

/// One bubble in the transcript.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub evidence: Vec<Evidence>,
    pub effects: Vec<String>,
}

/// `GET /api/assistant/status` and the status fields of a chat reply.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Status {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub hint: String,
}

/// `POST /api/assistant/chat`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub actions: Vec<UiAction>,
    #[serde(default)]
    pub error: String,
}

impl Reply {
    pub fn status(&self) -> Status {
        Status {
            configured: self.configured,
            provider: self.provider.clone(),
            model: self.model.clone(),
            hint: self.hint.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Panel {
    pub open: bool,
    pub draft: String,
    pub messages: Vec<Message>,
    pub status: Option<Status>,
    pub busy: bool,
    pub error: String,
    pub pending_hooks: Vec<UiAction>,
    pub generation: u64,
    /// Status has been requested at least once this page.
    pub status_fetched: bool,
    /// Transcript length last scrolled into view, so a new answer is shown
    /// from its first line rather than its trailing chips.
    scrolled_len: usize,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            draft: String::new(),
            messages: Vec::new(),
            status: None,
            busy: false,
            error: String::new(),
            pending_hooks: Vec::new(),
            generation: 0,
            status_fetched: false,
            scrolled_len: 0,
        }
    }
}

impl Panel {
    pub fn configured(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.configured)
    }

    /// JSON body for `POST /api/assistant/chat`. History is the transcript
    /// plus the new question. The capture itself is not in here.
    pub fn request_body(
        &self,
        intent: &str,
        text: &str,
        ranges: &[(u64, u64, Option<u32>)],
        scope: Option<(u32, &str)>,
        note: &str,
        pid: u32,
    ) -> String {
        let mut messages: Vec<serde_json::Value> = self
            .messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|m| serde_json::json!({"role": m.role, "content": m.text}))
            .collect();
        messages.push(serde_json::json!({"role": "user", "content": text}));
        let ranges: Vec<serde_json::Value> = ranges
            .iter()
            .map(|(start_ns, end_ns, tid)| serde_json::json!({"start_ns": start_ns, "end_ns": end_ns, "tid": tid}))
            .collect();
        serde_json::json!({
            "messages": messages,
            "ranges": ranges,
            "scope_id": scope.map(|(id, _)| id),
            "scope_name": scope.map(|(_, name)| name).unwrap_or(""),
            "intent": intent,
            "note": note,
            "pid": pid,
        })
        .to_string()
    }
}

/// What the panel asked the app to do.
#[derive(Clone, Debug)]
pub enum Event {
    Close,
    Send { intent: String, text: String },
    Apply(UiAction),
    ConfirmHook(usize),
    DismissHook(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Cite(String),
}

/// Splits `[[e12]]` citations out of an answer. Unknown brackets stay text.
pub fn split_cites(text: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut plain = String::new();
    while let Some(start) = rest.find("[[") {
        plain.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("]]") {
            let id = after[..end].trim();
            if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !id.is_empty() {
                if !plain.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut plain)));
                }
                out.push(Piece::Cite(id.to_string()));
            } else {
                plain.push_str("[[");
                plain.push_str(id);
                plain.push_str("]]");
            }
            rest = &after[end + 2..];
        } else {
            plain.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(Piece::Text(plain));
    }
    out
}

pub fn parse_status(text: &str) -> Result<Status, String> {
    serde_json::from_str(text).map_err(|e| format!("assistant status: {e}"))
}

pub fn parse_reply(text: &str) -> Result<Reply, String> {
    serde_json::from_str(text).map_err(|e| format!("assistant reply: {e}"))
}

#[cfg(feature = "egui")]
pub fn show(ctx: &egui::Context, panel: &mut Panel, narrow: bool) -> Vec<Event> {
    let mut events = Vec::new();
    let screen_h = ctx.screen_rect().height();
    let default_h = if narrow {
        (screen_h * 0.62).clamp(280.0, 680.0)
    } else {
        420.0
    };
    let margin = if narrow { 10.0 } else { 12.0 };
    egui::TopBottomPanel::bottom("orbit_assistant_panel")
        .resizable(true)
        .default_height(default_h)
        .min_height(if narrow { 200.0 } else { 168.0 })
        .max_height(screen_h * 0.86)
        .frame(
            Frame::new()
                .fill(crate::theme::PANEL())
                .inner_margin(Margin::symmetric(margin as i8, 8))
                .stroke(Stroke::new(1.0, crate::theme::HAIR())),
        )
        .show(ctx, |ui| {
            paint(ui, panel, narrow, &mut events);
        });
    events
}

#[cfg(feature = "egui")]
fn paint(ui: &mut Ui, panel: &mut Panel, narrow: bool, events: &mut Vec<Event>) {
    let button_h = if narrow { 34.0 } else { 24.0 };
    let body = if narrow { 13.0 } else { 12.5 };
    header(ui, panel, events);
    ui.add_space(4.0);

    let configured = panel.configured();
    let offline = panel.error.contains("no orbit-service");
    if !configured || offline {
        disabled(ui, panel, body);
    }

    if panel.busy {
        ui.label(
            RichText::new(thinking_label(ui.input(|i| i.time)))
                .color(crate::theme::ACCENT())
                .size(body),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(160));
    }

    let reserved = input_block_height(panel, button_h) + 8.0;
    let pinned = panel
        .messages
        .last()
        .filter(|message| message.role == "assistant")
        .cloned();
    let history_len = panel.messages.len() - usize::from(pinned.is_some());
    // The latest answer stays under the transcript, so a short panel shows the
    // sentence instead of only the last row of evidence chips.
    let scroll_h = if pinned.is_some() {
        (ui.available_height() * 0.22).clamp(56.0, 120.0)
    } else {
        (ui.available_height() - reserved).max(72.0)
    };
    let scroll_latest = panel.scrolled_len != history_len;
    ScrollArea::vertical()
        .id_salt("orbit_assistant_transcript")
        .auto_shrink([false, false])
        .max_height(scroll_h)
        .stick_to_bottom(false)
        .show(ui, |ui| {
            if panel.messages.is_empty() && configured && !offline {
                ui.label(
                    RichText::new("Ask about the capture in view. Answers cite the samples, scopes and thread states the service summarized. Nothing raw is sent.")
                        .color(crate::theme::MUTED())
                        .size(body),
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    quick(ui, events, "Explain this selection", "explain_selection", "Explain this selection.", button_h);
                    quick(ui, events, "Suggest optimizations", "suggest_optimizations", "What should I optimize?", button_h);
                    quick(ui, events, "Why is this slow?", "ask", "Why is this slow?", button_h);
                });
            }
            for (i, message) in panel.messages.iter().take(history_len).enumerate() {
                if scroll_latest && i + 1 == history_len {
                    ui.scroll_to_cursor(Some(Align::TOP));
                }
                bubble(ui, message, events, body, narrow);
                ui.add_space(6.0);
            }
            if pinned.is_none() && !panel.error.is_empty() && configured {
                ui.label(RichText::new(&panel.error).color(crate::theme::PLAYHEAD()).size(body));
            }
        });
    if scroll_latest {
        panel.scrolled_len = history_len;
    }
    if let Some(message) = pinned {
        ui.add_space(4.0);
        bubble(ui, &message, events, body, narrow);
        if !panel.error.is_empty() && configured {
            ui.label(
                RichText::new(&panel.error)
                    .color(crate::theme::PLAYHEAD())
                    .size(body),
            );
        }
    }

    hooks(ui, panel, events, button_h, body);
    if configured && !offline {
        input_row(ui, panel, events, button_h, narrow);
    }
}

#[cfg(feature = "egui")]
fn input_block_height(panel: &Panel, button_h: f32) -> f32 {
    let hooks = if panel.pending_hooks.is_empty() {
        0.0
    } else {
        panel.pending_hooks.len() as f32 * (button_h + 36.0)
    };
    hooks + button_h + 12.0
}

#[cfg(feature = "egui")]
fn header(ui: &mut Ui, panel: &Panel, events: &mut Vec<Event>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("ASSISTANT")
                .family(crate::fonts::medium())
                .size(11.0)
                .extra_letter_spacing(1.2)
                .color(crate::theme::ACCENT()),
        );
        let status = match &panel.status {
            Some(s) if s.configured && s.provider == "mock" => {
                "Demo model · on this machine".to_string()
            }
            Some(s) if s.configured => format!("{} · {}", s.provider, s.model),
            Some(_) => "Not configured".to_string(),
            None => "Checking the service…".to_string(),
        };
        let color = if panel.configured() {
            crate::theme::MUTED()
        } else {
            crate::theme::PLAYHEAD()
        };
        ui.label(RichText::new(status).color(color).size(11.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = ui.add(
                egui::Button::new(
                    RichText::new("Close")
                        .size(11.0)
                        .color(crate::theme::TEXT()),
                )
                .min_size(Vec2::new(52.0, 22.0))
                .fill(crate::theme::TRACK()),
            );
            crate::app::note_ui_rect("assistant:close", close.rect);
            if close.clicked() {
                events.push(Event::Close);
            }
        });
    });
}

#[cfg(feature = "egui")]
fn disabled(ui: &mut Ui, panel: &Panel, body: f32) {
    let hint = if panel.error.contains("no orbit-service") {
        panel.error.clone()
    } else if let Some(status) = &panel.status {
        if status.hint.is_empty() {
            "The assistant is not configured on this service.".into()
        } else {
            status.hint.clone()
        }
    } else if panel.error.is_empty() {
        "Checking whether orbit-service has a model configured…".into()
    } else {
        panel.error.clone()
    };
    ui.add_space(2.0);
    ui.label(RichText::new(hint).color(crate::theme::TEXT()).size(body));
    ui.add_space(6.0);
}

#[cfg(feature = "egui")]
fn quick(
    ui: &mut Ui,
    events: &mut Vec<Event>,
    label: &str,
    intent: &str,
    text: &str,
    button_h: f32,
) {
    let resp = ui.add(
        egui::Button::new(
            RichText::new(label)
                .family(crate::fonts::medium())
                .size(11.5)
                .color(crate::theme::TEXT()),
        )
        .fill(crate::theme::TRACK())
        .stroke(Stroke::new(1.0, crate::theme::HAIR()))
        .min_size(Vec2::new(0.0, button_h))
        .corner_radius(4),
    );
    crate::app::note_ui_rect(label, resp.rect);
    if resp.clicked() {
        events.push(Event::Send {
            intent: intent.into(),
            text: text.into(),
        });
    }
}

#[cfg(feature = "egui")]
fn bubble(ui: &mut Ui, message: &Message, events: &mut Vec<Event>, body: f32, narrow: bool) {
    let user = message.role == "user";
    let fill = if user {
        crate::theme::TRACK()
    } else {
        crate::theme::CANVAS()
    };
    let frame = Frame::new()
        .fill(fill)
        .inner_margin(Margin::symmetric(8, 6))
        .corner_radius(6.0)
        .stroke(Stroke::new(1.0, crate::theme::HAIR()));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let who = if user { "You" } else { "Assistant" };
        ui.label(
            RichText::new(who)
                .family(crate::fonts::medium())
                .size(10.0)
                .color(crate::theme::MUTED()),
        );
        cite_text(ui, &message.text, &message.evidence, events, body);
        if !message.evidence.is_empty() && !user {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for ev in &message.evidence {
                    let label = format!("{}  {}", ev.id, ev.label);
                    let resp = ui
                        .add(
                            egui::Button::new(
                                RichText::new(label)
                                    .font(FontId::monospace(if narrow { 11.0 } else { 10.5 }))
                                    .color(crate::theme::ACCENT()),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::new(1.0, crate::theme::HAIR()))
                            .min_size(Vec2::new(0.0, if narrow { 30.0 } else { 20.0 }))
                            .corner_radius(4),
                        )
                        .on_hover_text(&ev.detail);
                    if resp.clicked() {
                        if let Some(action) = &ev.action {
                            events.push(Event::Apply(action.clone()));
                        }
                    }
                }
            });
        }
        if !message.effects.is_empty() {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("Viewer: {}", message.effects.join(" · ")))
                    .color(crate::theme::MUTED())
                    .size(body - 1.0),
            );
        }
    });
}

#[cfg(feature = "egui")]
fn cite_text(ui: &mut Ui, text: &str, evidence: &[Evidence], events: &mut Vec<Event>, body: f32) {
    let pieces = split_cites(text);
    if pieces.iter().all(|p| matches!(p, Piece::Text(_))) {
        ui.label(RichText::new(text).color(crate::theme::TEXT()).size(body));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    ui.label(RichText::new(text).color(crate::theme::TEXT()).size(body));
                }
                Piece::Cite(id) => {
                    let ev = evidence.iter().find(|e| e.id == id);
                    let tip = ev
                        .map(|e| format!("{} — {}", e.label, e.detail))
                        .unwrap_or_else(|| id.clone());
                    let resp = ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("[{id}]"))
                                    .font(FontId::monospace(body - 1.0))
                                    .color(crate::theme::ACCENT()),
                            )
                            .fill(crate::theme::INPUT())
                            .stroke(Stroke::NONE)
                            .min_size(Vec2::new(0.0, body + 6.0))
                            .corner_radius(3),
                        )
                        .on_hover_text(tip);
                    if resp.clicked() {
                        if let Some(action) = ev.and_then(|e| e.action.clone()) {
                            events.push(Event::Apply(action));
                        }
                    }
                }
            }
        }
    });
}

#[cfg(feature = "egui")]
fn hooks(ui: &mut Ui, panel: &Panel, events: &mut Vec<Event>, button_h: f32, body: f32) {
    for (i, action) in panel.pending_hooks.iter().enumerate() {
        let UiAction::SuggestHook { name, reason, .. } = action else {
            continue;
        };
        ui.add_space(4.0);
        Frame::new()
            .fill(crate::theme::INPUT())
            .inner_margin(Margin::symmetric(8, 6))
            .corner_radius(6.0)
            .stroke(Stroke::new(1.0, crate::theme::PLAYHEAD()))
            .show(ui, |ui| {
                ui.label(RichText::new(format!("Hook {name} on the next Record?")).color(crate::theme::TEXT()).size(body));
                ui.label(RichText::new(reason).color(crate::theme::MUTED()).size(body - 1.0));
                ui.label(
                    RichText::new("Nothing is hooked until you confirm. The next Record arms it; this capture stays as it is.")
                        .color(crate::theme::MUTED())
                        .size(body - 1.5),
                );
                ui.horizontal(|ui| {
                    let ok = ui.add(
                        egui::Button::new(RichText::new("Hook it").family(crate::fonts::medium()).size(12.0).color(crate::theme::CANVAS()))
                            .fill(crate::theme::ACCENT())
                            .min_size(Vec2::new(72.0, button_h)),
                    );
                    crate::app::note_ui_rect("assistant:hook", ok.rect);
                    if ok.clicked() {
                        events.push(Event::ConfirmHook(i));
                    }
                    let no = ui.add(
                        egui::Button::new(RichText::new("Not now").size(12.0).color(crate::theme::TEXT()))
                            .fill(crate::theme::TRACK())
                            .min_size(Vec2::new(72.0, button_h)),
                    );
                    if no.clicked() {
                        events.push(Event::DismissHook(i));
                    }
                });
            });
    }
}

#[cfg(feature = "egui")]
fn input_row(ui: &mut Ui, panel: &mut Panel, events: &mut Vec<Event>, button_h: f32, narrow: bool) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let send_w = if narrow { 64.0 } else { 56.0 };
        let edit = ui.add(
            TextEdit::singleline(&mut panel.draft)
                .id_salt("orbit_assistant_draft")
                .desired_width((ui.available_width() - send_w - 8.0).max(80.0))
                .hint_text("Ask about this capture")
                .font(FontId::proportional(if narrow { 14.0 } else { 13.0 }))
                .margin(Margin::symmetric(8, 6))
                .background_color(crate::theme::INPUT()),
        );
        let enter =
            edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        let send = ui.add_enabled(
            !panel.busy && !panel.draft.trim().is_empty(),
            egui::Button::new(
                RichText::new("Send")
                    .family(crate::fonts::medium())
                    .size(12.0)
                    .color(crate::theme::CANVAS()),
            )
            .fill(crate::theme::ACCENT())
            .min_size(Vec2::new(send_w, button_h)),
        );
        crate::app::note_ui_rect("assistant:send", send.rect);
        if (send.clicked() || enter) && !panel.busy {
            let text = panel.draft.trim().to_string();
            if !text.is_empty() {
                panel.draft.clear();
                events.push(Event::Send {
                    intent: "ask".into(),
                    text,
                });
            }
        }
    });
}

#[cfg(feature = "egui")]
fn thinking_label(now_s: f64) -> String {
    let dots = (now_s * 2.0) as usize % 4;
    format!("Looking at the capture{}", ".".repeat(dots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_split_out_and_unknown_brackets_stay_text() {
        let pieces = split_cites("hot is [[e2]] and [[not an id!]] stays.");
        assert_eq!(
            pieces,
            vec![
                Piece::Text("hot is ".into()),
                Piece::Cite("e2".into()),
                Piece::Text(" and [[not an id!]] stays.".into()),
            ]
        );
    }

    #[test]
    fn a_reply_round_trips_actions_and_a_missing_model_is_not_configured() {
        let reply = parse_reply(
            r#"{"configured":true,"provider":"mock","model":"orbit-mock","message":"See [[e2]].","evidence":[{"id":"e2","label":"Game::Update","detail":"40% self","action":{"type":"jump_to_function","name":"Game::Update","function_id":88}}],"actions":[{"type":"suggest_hook","name":"Game::Update","function_id":88,"module":"game","reason":"40% self"}],"error":""}"#,
        )
        .unwrap();
        assert!(reply.actions[0].needs_confirmation());
        assert_eq!(reply.evidence[0].label, "Game::Update");
        let status = parse_status(
            r#"{"configured":false,"hint":"Set ORBIT_AI_API_KEY","provider":"","model":""}"#,
        )
        .unwrap();
        assert!(!status.configured);
        assert!(status.hint.contains("ORBIT_AI_API_KEY"));
    }

    #[test]
    fn the_request_carries_the_question_and_not_a_key() {
        let mut panel = Panel::default();
        panel.messages.push(Message {
            role: "user".into(),
            text: "earlier".into(),
            evidence: Vec::new(),
            effects: Vec::new(),
        });
        panel.messages.push(Message {
            role: "assistant".into(),
            text: "ok".into(),
            evidence: Vec::new(),
            effects: Vec::new(),
        });
        let body = panel.request_body(
            "explain_selection",
            "Explain this selection.",
            &[(10, 20, Some(7))],
            None,
            "",
            4,
        );
        assert!(body.contains("Explain this selection."));
        assert!(body.contains("earlier"));
        assert!(body.contains("\"start_ns\":10"));
        assert!(!body.contains("api_key"));
        assert!(!body.contains("sk-"));
    }
}
