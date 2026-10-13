// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Capture summaries and the assistant's tools.
//!
//! The live viewer's Ask panel ([`complete`]) and any other agent — including
//! a Model Context Protocol server added alongside this one — should share
//! this crate instead of folding the capture a second time.
//!
//! Start from [`facts_from_index`] or [`hotspots_from_report_json`], then
//! [`summarize`] for the token-budgeted text. [`tool_specs`] is the tool
//! list (OpenAI and Anthropic shapes are [`openai_tools`] and
//! [`anthropic_tools`]). [`dispatch_tool`] turns a model tool call into a
//! viewer action or a data lookup. [`complete`] runs the tool loop against
//! an OpenAI-compatible endpoint or Anthropic.
//!
//! Capture data in the summary is sent to whatever provider is configured.
//! The API key is read on the service (`ORBIT_AI_API_KEY`) and is not part of
//! this crate's public summary.

mod evidence;
mod facts;
mod provider;
mod summary;
mod tools;

#[cfg(any(test, feature = "mock-server"))]
pub mod testing;

pub use evidence::{message_segments, parse_evidence_link, EvidenceTarget, Segment};
pub use facts::{
    disasm_from_json, facts_from_index, hotspots_from_report_json, CallstackRow, CaptureFacts, DisasmSnippet, Hotspot,
    ScopeStat, ThreadStateShare, TimeRange,
};
pub use provider::{
    complete_with, config_from, config_from_env, disabled_status, AiConfig, AiEnv, AiError, AiStatus, ChatRequest, ChatResponse,
    FactsHost, InsightHost, Intent, ModelClient, ProviderKind, Turn,
};
pub use summary::{estimate_tokens, linkify, summarize, Summary, TokenBudget};
pub use tools::{
    anthropic_tools, dispatch_tool, openai_tools, tool_specs, answer_data_tool, ToolCall, ToolOutcome, UiAction,
};

#[cfg(feature = "provider")]
pub use provider::{complete, UreqClient};
