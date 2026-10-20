// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The tools an agent calls, and the small HTTP client they share.
//!
//! State that the service does not keep — which process is attached, which
//! functions the next capture should hook — lives here, in the MCP process.
//! Hooks are installed only when a capture starts (`instrumented_functions`
//! on `POST /api/capture/start`). Arming and unhooking edit that set.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

/// Matches `hooks::MAX_HOOKS`. The service silently keeps the first 16;
/// the tool says so before that happens.
const MAX_HOOKS: usize = 16;
const SEARCH_CAP: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    pub name: &'static str,
    pub sampling: bool,
    pub scheduler: bool,
    pub kernel_uprobes: bool,
    pub auto_profile: bool,
    pub disassembly: bool,
    pub dynamic_hooks: bool,
}

impl Platform {
    pub fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::macos()
        } else if cfg!(target_os = "linux") {
            Self::linux()
        } else if cfg!(target_os = "windows") {
            Self::windows()
        } else {
            Self {
                name: "other",
                sampling: false,
                scheduler: false,
                kernel_uprobes: false,
                auto_profile: false,
                disassembly: false,
                dynamic_hooks: false,
            }
        }
    }

    pub fn linux() -> Self {
        Self {
            name: "linux",
            sampling: true,
            scheduler: true,
            kernel_uprobes: true,
            auto_profile: true,
            disassembly: true,
            dynamic_hooks: true,
        }
    }

    /// Windows: the kernel logger gives sampling, the scheduler and thread
    /// states (with an elevated prompt); Frida hooks and manual scopes
    /// need no privilege. Kernel uprobes are Linux, and auto-profiling
    /// rides on them.
    pub fn windows() -> Self {
        Self {
            name: "windows",
            sampling: true,
            scheduler: true,
            kernel_uprobes: false,
            auto_profile: false,
            disassembly: true,
            dynamic_hooks: true,
        }
    }

    pub fn macos() -> Self {
        Self {
            name: "macos",
            sampling: false,
            scheduler: false,
            kernel_uprobes: false,
            auto_profile: false,
            disassembly: false,
            dynamic_hooks: true,
        }
    }
}

#[derive(Clone, Debug)]
struct Armed {
    function_id: u64,
    name: String,
    module: String,
}

struct Session {
    pid: Option<u32>,
    hooks: Vec<Armed>,
    /// Names learned from `orbit_search_functions`, so an id the agent
    /// passes back still has a name. The service has no id lookup.
    known: HashMap<u64, Armed>,
}

pub struct Mcp {
    http: HttpClient,
    session: Mutex<Session>,
    platform: Platform,
}

impl Mcp {
    pub fn connect(url: &str, allow_remote: bool, platform: Platform) -> Result<Self, String> {
        Ok(Self {
            http: HttpClient::parse(url, allow_remote)?,
            session: Mutex::new(Session { pid: None, hooks: Vec::new(), known: HashMap::new() }),
            platform,
        })
    }

    /// One JSON-RPC message. `None` when the message is a notification.
    pub fn handle(&self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        if id.is_none() {
            return None;
        }
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_specs() })),
            "tools/call" => self.tools_call(&params),
            _ => Err(rpc_error(-32601, &format!("unknown method {method}"))),
        };
        Some(match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-03-26");
        let version = if matches!(requested, "2024-11-05" | "2025-03-26" | "2025-06-18") {
            requested
        } else {
            "2025-03-26"
        };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "orbit", "version": "0.1.0" },
            "instructions": INSTRUCTIONS,
        })
    }

    fn tools_call(&self, params: &Value) -> Result<Value, Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        if name.is_empty() {
            return Err(rpc_error(-32602, "tools/call needs a name"));
        }
        match self.dispatch(name, &args) {
            Ok(payload) => Ok(tool_ok(&payload)),
            Err(message) => Ok(tool_err(&message)),
        }
    }

    fn dispatch(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "orbit_capabilities" => self.capabilities(),
            "orbit_list_processes" => self.list_processes(args),
            "orbit_attach" => self.attach(args),
            "orbit_search_functions" => self.search_functions(args),
            "orbit_arm_hooks" => self.arm_hooks(args),
            "orbit_unhook" => self.unhook(args),
            "orbit_start_capture" => self.start_capture(args),
            "orbit_stop_capture" => self.stop_capture(),
            "orbit_capture_status" => self.capture_status(),
            "orbit_sampling_report" => self.sampling_report(args),
            "orbit_callstacks" => self.callstacks(args),
            "orbit_function_stats" => self.function_stats(args),
            _ => Err(format!("unknown tool {name}")),
        }
    }

    fn capabilities(&self) -> Result<Value, String> {
        let (reachable, capture, error) = match self.http.get_json("/api/status", Duration::from_secs(2)) {
            Ok(status) => (true, Some(summarize_status(&status)), None),
            Err(error) => (false, None, Some(error)),
        };
        let session = self.session.lock().map_err(|_| "session poisoned".to_string())?;
        let p = self.platform;
        let engines: Vec<&str> = if p.kernel_uprobes {
            vec!["frida", "kernel_uprobes"]
        } else if p.dynamic_hooks {
            vec!["frida"]
        } else {
            vec![]
        };
        Ok(json!({
            "platform": p.name,
            "service_url": self.http.display,
            "service_reachable": reachable,
            "service_error": error,
            "features": {
                "sampling": feature(p.sampling, "Call-stack sampling. Linux and Windows (elevated prompt); macOS has no sampler."),
                "scheduler": feature(p.scheduler, "Context switches and thread states. Linux and Windows (elevated prompt)."),
                "dynamic_hooks": {
                    "supported": p.dynamic_hooks,
                    "engines": engines,
                    "detail": "Frida hooks a running process with no rebuild. kernel_uprobes is Linux-only and needs root or CAP_SYS_ADMIN; it is never the default.",
                },
                "auto_profile": feature(p.auto_profile, "The service chooses hooks from samples and prunes them to a budget. Linux only, off unless requested."),
                "disassembly": feature(p.disassembly, "Disassembly next to source. Linux and Windows (no source lines from a PDB yet)."),
                "function_stats": feature(true, "Count, total, average, min, max and standard deviation for hooks and manual scopes."),
            },
            "session": {
                "attached_pid": session.pid,
                "armed_hooks": session.hooks.len(),
                "armed": session.hooks.iter().take(16).map(armed_json).collect::<Vec<_>>(),
            },
            "capture": capture,
            "limits": { "max_hooks": MAX_HOOKS },
            "auth": "orbit-service has no authentication, and this server does not add any. It talks to loopback unless started with --allow-remote, and its HTTP transport binds to loopback only. Keep the service on 127.0.0.1.",
        }))
    }

    fn list_processes(&self, args: &Value) -> Result<Value, String> {
        let limit = page_limit(args, 20, 50);
        let offset = page_offset(args);
        let query = arg_str(args, "query").unwrap_or("").to_ascii_lowercase();
        let raw = self.http.get_json("/api/processes", Duration::from_secs(10))?;
        let list = raw.as_array().cloned().unwrap_or_default();
        let matched: Vec<&Value> = list
            .iter()
            .filter(|row| {
                query.is_empty()
                    || row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase().contains(&query)
            })
            .collect();
        let total = matched.len();
        let page: Vec<Value> = matched
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| {
                json!({
                    "pid": row.get("pid").and_then(|v| v.as_u64()).unwrap_or(0),
                    "name": row.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "cpu_percent": round1(row.get("cpu").and_then(|v| v.as_f64()).unwrap_or(0.0)),
                })
            })
            .collect();
        Ok(json!({
            "total": total,
            "offset": offset,
            "limit": limit,
            "processes": page,
        }))
    }

    fn attach(&self, args: &Value) -> Result<Value, String> {
        let pid = arg_u32(args, "pid").ok_or("pid is required")?;
        let timeout = Duration::from_millis(arg_u64(args, "timeout_ms").unwrap_or(30_000).min(120_000));
        self.http.post_ok("/api/symbols/load", &json!({ "pid": pid }), Duration::from_secs(30))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let status = self.http.get_json(&format!("/api/symbols/status?pid={pid}"), Duration::from_secs(10))?;
            let state = status.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if state == "ready" {
                let mut session = self.session.lock().map_err(|_| "session poisoned".to_string())?;
                let switched = session.pid != Some(pid);
                let had_hooks = !session.hooks.is_empty();
                if switched {
                    session.hooks.clear();
                    session.known.clear();
                }
                session.pid = Some(pid);
                return Ok(json!({
                    "attached": true,
                    "pid": pid,
                    "function_count": status.get("function_count").and_then(|v| v.as_u64()).unwrap_or(0),
                    "module_count": status.get("module_count").and_then(|v| v.as_u64()).unwrap_or(0),
                    "hooks_cleared": switched && had_hooks,
                }));
            }
            if state == "error" {
                let error = status.get("error").and_then(|v| v.as_str()).unwrap_or("symbol load failed");
                return Err(error.to_string());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("symbols for pid {pid} were not ready within {} ms (status {state})", timeout.as_millis()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn search_functions(&self, args: &Value) -> Result<Value, String> {
        let pid = self.pid_of(args)?;
        let query = arg_str(args, "query").unwrap_or("");
        let limit = page_limit(args, 15, 40);
        let offset = page_offset(args);
        if offset + limit > SEARCH_CAP {
            return Err(format!(
                "search pages at most {SEARCH_CAP} hits. Narrow query instead of offset {offset}."
            ));
        }
        let ask = offset + limit;
        let path = format!(
            "/api/functions/search?pid={pid}&q={}&limit={ask}",
            urlencode(query)
        );
        let body = self.http.get_json(&path, Duration::from_secs(30))?;
        let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status != "ready" && status != "" {
            return Err(format!("symbols are {status}; call orbit_attach and wait until it returns"));
        }
        let functions = body.get("functions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        {
            let mut session = self.session.lock().map_err(|_| "session poisoned".to_string())?;
            for row in &functions {
                let hit = armed_from(row);
                if hit.function_id != 0 {
                    session.known.insert(hit.function_id, hit);
                }
            }
        }
        let more = functions.len() >= ask;
        let page: Vec<Value> = functions
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| {
                json!({
                    "function_id": row.get("function_id").and_then(json_u64).unwrap_or(0),
                    "name": clip(row.get("name").and_then(|v| v.as_str()).unwrap_or("")),
                    "module": basename(row.get("module").and_then(|v| v.as_str()).unwrap_or("")),
                    "size": row.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    "safety": row.get("safety").and_then(|v| v.as_str()).unwrap_or("unknown"),
                })
            })
            .collect();
        Ok(json!({
            "pid": pid,
            "offset": offset,
            "limit": limit,
            "more": more,
            "index_size": body.get("total").and_then(|v| v.as_u64()).unwrap_or(0),
            "functions": page,
        }))
    }

    fn arm_hooks(&self, args: &Value) -> Result<Value, String> {
        let pid = self.pid_of(args)?;
        let mode = arg_str(args, "mode").unwrap_or("add");
        if mode != "add" && mode != "replace" {
            return Err("mode is add or replace".into());
        }
        let mut resolved = Vec::new();
        let mut ambiguous = Vec::new();
        let mut missing = Vec::new();
        for id in arg_u64s(args, "function_ids") {
            match self.lookup_id(pid, id)? {
                Some(hit) => resolved.push(hit),
                None => missing.push(id.to_string()),
            }
        }
        let module = arg_str(args, "module").unwrap_or("");
        for name in arg_strs(args, "names") {
            match self.resolve_name(pid, &name, module)? {
                NameHit::One(hit) => resolved.push(hit),
                NameHit::None => missing.push(name),
                NameHit::Many(modules) => ambiguous.push(json!({ "name": name, "modules": modules })),
            }
        }
        if resolved.is_empty() && ambiguous.is_empty() && missing.is_empty() {
            return Err("pass function_ids or names".into());
        }
        let mut session = self.session.lock().map_err(|_| "session poisoned".to_string())?;
        if mode == "replace" {
            session.hooks.clear();
        }
        for hit in resolved {
            if session.hooks.iter().any(|h| h.function_id == hit.function_id) {
                continue;
            }
            session.hooks.push(hit);
        }
        let dropped = session.hooks.len().saturating_sub(MAX_HOOKS);
        session.hooks.truncate(MAX_HOOKS);
        let capturing = self.capturing().unwrap_or(false);
        Ok(json!({
            "pid": pid,
            "armed": session.hooks.len(),
            "hooks": session.hooks.iter().map(armed_json).collect::<Vec<_>>(),
            "dropped_over_cap": dropped,
            "missing": missing,
            "ambiguous": ambiguous,
            "applies": if capturing { "next_capture" } else { "next_start" },
            "capturing": capturing,
            "note": "Hooks are installed when orbit_start_capture runs. This call does not modify the target.",
        }))
    }

    fn unhook(&self, args: &Value) -> Result<Value, String> {
        let all = arg_bool(args, "all").unwrap_or(false);
        let ids = arg_u64s(args, "function_ids");
        let names = arg_strs(args, "names");
        if !all && ids.is_empty() && names.is_empty() {
            return Err("pass function_ids, names, or all:true".into());
        }
        let mut session = self.session.lock().map_err(|_| "session poisoned".to_string())?;
        let before = session.hooks.len();
        if all {
            session.hooks.clear();
        } else {
            session.hooks.retain(|hook| {
                !ids.contains(&hook.function_id) && !names.iter().any(|name| name == &hook.name)
            });
        }
        let capturing = self.capturing().unwrap_or(false);
        Ok(json!({
            "removed": before - session.hooks.len(),
            "armed": session.hooks.len(),
            "hooks": session.hooks.iter().map(armed_json).collect::<Vec<_>>(),
            "capturing": capturing,
            "note": if capturing {
                "The running capture keeps the hooks it started with. Stop it and start again to apply this set."
            } else {
                "The next capture will not instrument the removed functions."
            },
        }))
    }

    fn start_capture(&self, args: &Value) -> Result<Value, String> {
        let pid = self.pid_of(args)?;
        let mut unsupported = Vec::new();
        if arg_bool(args, "sampling") == Some(true) && !self.platform.sampling {
            unsupported.push("sampling");
        }
        if arg_bool(args, "scheduler") == Some(true) && !self.platform.scheduler {
            unsupported.push("scheduler");
        }
        if arg_bool(args, "auto_profile") == Some(true) && !self.platform.auto_profile {
            unsupported.push("auto_profile");
        }
        let instrumentation = arg_str(args, "instrumentation").unwrap_or("frida");
        if instrumentation == "kernel_uprobes" && !self.platform.kernel_uprobes {
            unsupported.push("kernel_uprobes");
        }
        if instrumentation != "frida" && instrumentation != "kernel_uprobes" {
            return Err("instrumentation is frida or kernel_uprobes".into());
        }
        if !unsupported.is_empty() {
            return Ok(json!({
                "started": false,
                "supported": false,
                "platform": self.platform.name,
                "unsupported": unsupported,
                "message": format!(
                    "{} orbit-service does not support {}. Start again without those fields. Function timing from Frida hooks still works via orbit_function_stats.",
                    self.platform.name,
                    unsupported.join(", ")
                ),
            }));
        }
        let sampling = arg_bool(args, "sampling").unwrap_or(self.platform.sampling);
        let scheduler = arg_bool(args, "scheduler").unwrap_or(self.platform.scheduler);
        let auto_profile = arg_bool(args, "auto_profile").unwrap_or(false);
        let rate = arg_f64(args, "samples_per_second").unwrap_or(1000.0).clamp(1.0, 10_000.0);
        let hooks = self.session.lock().map_err(|_| "session poisoned".to_string())?.hooks.clone();
        let body = json!({
            "pid": pid,
            "enable_api": true,
            "sampling": sampling,
            "context_switches": scheduler,
            "thread_states": scheduler,
            "samples_per_second": rate,
            "dynamic_instrumentation_method": instrumentation,
            "instrumented_functions": hooks.iter().map(|h| json!({"function_id": h.function_id})).collect::<Vec<_>>(),
            "auto_profile": auto_profile,
        });
        self.http.post_ok("/api/capture/start", &body, Duration::from_secs(120))?;
        Ok(json!({
            "started": true,
            "supported": true,
            "pid": pid,
            "sampling": sampling,
            "scheduler": scheduler,
            "samples_per_second": rate,
            "instrumentation": instrumentation,
            "auto_profile": auto_profile,
            "hooks": hooks.len(),
            "hook_names": hooks.iter().map(|h| clip(&h.name)).collect::<Vec<_>>(),
        }))
    }

    fn stop_capture(&self) -> Result<Value, String> {
        self.http.post_ok("/api/capture/stop", &json!({}), Duration::from_secs(60))?;
        Ok(json!({ "stopped": true }))
    }

    fn capture_status(&self) -> Result<Value, String> {
        let status = self.http.get_json("/api/status", Duration::from_secs(5))?;
        Ok(summarize_status(&status))
    }

    fn sampling_report(&self, args: &Value) -> Result<Value, String> {
        if !self.platform.sampling {
            return Ok(unsupported_feature("sampling"));
        }
        let limit = page_limit(args, 15, 40);
        let offset = page_offset(args);
        match self.http.get_raw("/api/sampling/report", Duration::from_secs(60)) {
            Ok(raw) if raw.status == 501 => Ok(unsupported_feature("sampling")),
            Ok(raw) => {
                let body = parse_body(&raw)?;
                let functions = body.get("functions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let total = functions.len();
                let hotspots: Vec<Value> = functions
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .map(|row| {
                        let id = row.get("function_id").and_then(json_u64).unwrap_or(0);
                        json!({
                            "name": clip(row.get("name").and_then(|v| v.as_str()).unwrap_or("")),
                            "module": basename(row.get("module").and_then(|v| v.as_str()).unwrap_or("")),
                            "function_id": if id == 0 { Value::Null } else { json!(id) },
                            "self": row.get("self").and_then(|v| v.as_u64()).unwrap_or(0),
                            "inclusive": row.get("inclusive").and_then(|v| v.as_u64()).unwrap_or(0),
                            "self_percent": round1(row.get("self_percent").and_then(|v| v.as_f64()).unwrap_or(0.0)),
                            "inclusive_percent": round1(row.get("inclusive_percent").and_then(|v| v.as_f64()).unwrap_or(0.0)),
                        })
                    })
                    .collect();
                Ok(json!({
                    "supported": true,
                    "samples": body.get("samples").and_then(|v| v.as_u64()).unwrap_or(0),
                    "total_functions": total,
                    "offset": offset,
                    "limit": limit,
                    "hotspots": hotspots,
                }))
            }
            Err(error) => Err(error),
        }
    }

    fn callstacks(&self, args: &Value) -> Result<Value, String> {
        if !self.platform.sampling {
            return Ok(unsupported_feature("callstacks"));
        }
        let limit = page_limit(args, 8, 20);
        let offset = page_offset(args);
        let path = format!("/api/sampling/callstacks?offset={offset}&limit={limit}");
        match self.http.get_raw(&path, Duration::from_secs(60)) {
            Ok(raw) if raw.status == 501 => Ok(unsupported_feature("callstacks")),
            Ok(raw) => {
                let mut body = parse_body(&raw)?;
                if let Some(stacks) = body.get_mut("stacks").and_then(|v| v.as_array_mut()) {
                    for stack in stacks {
                        if let Some(frames) = stack.get_mut("frames").and_then(|v| v.as_array_mut()) {
                            for frame in frames {
                                if let Some(text) = frame.as_str() {
                                    *frame = json!(clip(text));
                                }
                            }
                        }
                    }
                }
                if let Some(map) = body.as_object_mut() {
                    map.insert("supported".into(), json!(true));
                }
                Ok(body)
            }
            Err(error) => Err(error),
        }
    }

    fn function_stats(&self, args: &Value) -> Result<Value, String> {
        let limit = page_limit(args, 15, 40);
        let offset = page_offset(args);
        let order = arg_str(args, "order").unwrap_or("total");
        if !matches!(order, "total" | "count" | "avg" | "max") {
            return Err("order is total, count, avg, or max".into());
        }
        let mut path = format!("/api/functions/stats?offset={offset}&limit={limit}&order={order}");
        if let Some(query) = arg_str(args, "query") {
            if !query.is_empty() {
                path.push_str("&q=");
                path.push_str(&urlencode(query));
            }
        }
        if let Some(pid) = arg_u32(args, "pid") {
            path.push_str(&format!("&pid={pid}"));
        }
        let mut body = self.http.get_json(&path, Duration::from_secs(60))?;
        if let Some(rows) = body.get_mut("rows").and_then(|v| v.as_array_mut()) {
            for row in rows {
                if let Some(name) = row.get("name").and_then(|v| v.as_str()) {
                    let clipped = clip(name);
                    if let Some(map) = row.as_object_mut() {
                        map.insert("name".into(), json!(clipped));
                    }
                }
            }
        }
        Ok(body)
    }

    fn pid_of(&self, args: &Value) -> Result<u32, String> {
        if let Some(pid) = arg_u32(args, "pid") {
            return Ok(pid);
        }
        self.session
            .lock()
            .map_err(|_| "session poisoned".to_string())?
            .pid
            .ok_or_else(|| "no process attached; call orbit_attach or pass pid".into())
    }

    fn capturing(&self) -> Result<bool, String> {
        let status = self.http.get_json("/api/status", Duration::from_secs(2))?;
        Ok(status.get("capturing").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    fn lookup_id(&self, _pid: u32, id: u64) -> Result<Option<Armed>, String> {
        let session = self.session.lock().map_err(|_| "session poisoned".to_string())?;
        if let Some(hit) = session.known.get(&id) {
            return Ok(Some(hit.clone()));
        }
        // The id was not on a page this session searched. Keep it: the
        // service resolves ids itself and skips one it does not know.
        Ok(Some(Armed { function_id: id, name: format!("function {id}"), module: String::new() }))
    }

    fn resolve_name(&self, pid: u32, name: &str, module: &str) -> Result<NameHit, String> {
        let path = format!("/api/functions/search?pid={pid}&q={}&limit=20", urlencode(name));
        let body = self.http.get_json(&path, Duration::from_secs(30))?;
        let functions = body.get("functions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let exact: Vec<&Value> = functions
            .iter()
            .filter(|row| row.get("name").and_then(|v| v.as_str()) == Some(name))
            .filter(|row| module.is_empty() || basename(row.get("module").and_then(|v| v.as_str()).unwrap_or("")) == module)
            .collect();
        match exact.as_slice() {
            [] => Ok(NameHit::None),
            [one] => Ok(NameHit::One(armed_from(one))),
            many => Ok(NameHit::Many(
                many.iter()
                    .map(|row| basename(row.get("module").and_then(|v| v.as_str()).unwrap_or("")).to_string())
                    .collect(),
            )),
        }
    }
}

enum NameHit {
    One(Armed),
    None,
    Many(Vec<String>),
}

fn armed_from(row: &Value) -> Armed {
    Armed {
        function_id: row.get("function_id").and_then(json_u64).unwrap_or(0),
        name: row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        module: basename(row.get("module").and_then(|v| v.as_str()).unwrap_or("")).to_string(),
    }
}

fn armed_json(hook: &Armed) -> Value {
    json!({
        "function_id": hook.function_id,
        "name": clip(&hook.name),
        "module": hook.module,
    })
}

fn feature(supported: bool, detail: &str) -> Value {
    json!({ "supported": supported, "detail": detail })
}

fn unsupported_feature(feature: &str) -> Value {
    json!({
        "supported": false,
        "feature": feature,
        "message": format!(
            "{feature} is not available on this platform. macOS records Frida hooks and manual scopes; read those with orbit_function_stats."
        ),
    })
}

fn summarize_status(status: &Value) -> Value {
    let instrumentation = status.get("instrumentation").and_then(|v| v.as_str()).unwrap_or("");
    let crash = status.get("hook_crash").and_then(|v| v.as_str()).unwrap_or("");
    json!({
        "capturing": status.get("capturing").and_then(|v| v.as_bool()).unwrap_or(false),
        "target_pid": status.get("target_pid").and_then(|v| v.as_u64()).unwrap_or(0),
        "events": status.get("events_live").and_then(|v| v.as_u64()).unwrap_or(0),
        "dropped": status.get("dropped").and_then(|v| v.as_u64()).unwrap_or(0),
        "instrumentation": clip_len(instrumentation, 300),
        "hook_crash": if crash.is_empty() { Value::Null } else { json!(clip_len(crash, 300)) },
        "capture_start_ns": status.get("capture_start_ns").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

const INSTRUCTIONS: &str = "\
Orbit profiles a running process. The service must already be running \
(default http://127.0.0.1:44766). Call orbit_capabilities first. \
Typical loop: orbit_list_processes, orbit_attach, orbit_search_functions, \
orbit_arm_hooks, orbit_start_capture, orbit_stop_capture, then \
orbit_function_stats and, on Linux, orbit_sampling_report and orbit_callstacks. \
Change the hook set with orbit_unhook and start again; a running capture \
keeps the hooks it started with. Sampling and the scheduler are Linux-only; \
on macOS those tools say so and do not invent an empty profile. \
This server does not edit source, rebuild, kill processes, or delete files. \
The HTTP API has no authentication.";

fn tool_ok(payload: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": payload.to_string() }],
        "isError": false,
    })
}

fn tool_err(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": json!({"error": message}).to_string() }],
        "isError": true,
    })
}

fn rpc_error(code: i32, message: &str) -> Value {
    json!({ "code": code, "message": message })
}

pub fn tool_specs() -> Vec<Value> {
    vec![
        tool(
            "orbit_capabilities",
            "Read this first. Platform, which profiling features this machine supports, whether orbit-service is reachable, and the session (attached pid, armed hooks). Sampling, the scheduler, kernel uprobes and auto-profiling are Linux-only. macOS supports Frida hooks and manual scopes. Does not modify anything.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_list_processes",
            "List processes orbit-service can see, busiest first. query is a case-insensitive substring of the process name. One page of pid, name and cpu_percent. Pass a pid to orbit_attach.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Substring of the process name. Empty lists the busiest." },
                    "limit": { "type": "integer", "description": "Page size. Default 20, max 50." },
                    "offset": { "type": "integer", "description": "Rows to skip. Default 0." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_attach",
            "Load symbols for pid and remember it as the target. Waits until symbols are ready or timeout_ms (default 30000, max 120000). Switching pid clears hooks armed for the previous process. Does not start a capture and does not modify the target.",
            json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "description": "Process id from orbit_list_processes." },
                    "timeout_ms": { "type": "integer", "description": "How long to wait for symbols. Default 30000." }
                },
                "required": ["pid"],
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_search_functions",
            "Search the attached process for functions to hook. Returns one page of function_id, name, module, size and hook safety. The service ranks hits; offset pages that ranking and must stay within 200. Pass function_id to orbit_arm_hooks. Requires orbit_attach, or pid.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol text to search for." },
                    "pid": { "type": "integer", "description": "Defaults to the attached process." },
                    "limit": { "type": "integer", "description": "Page size. Default 15, max 40." },
                    "offset": { "type": "integer", "description": "Hits to skip. offset + limit must be <= 200." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_arm_hooks",
            "Choose functions the next capture will instrument. Pass function_ids from orbit_search_functions, or exact names (narrow with module when a name exists in more than one). mode add (default) or replace. At most 16 functions; extras are counted in dropped_over_cap and not armed. Does not modify the running process. A capture already running keeps the set it started with until you stop and start again.",
            json!({
                "type": "object",
                "properties": {
                    "function_ids": { "type": "array", "items": { "type": "integer" }, "description": "Ids from orbit_search_functions." },
                    "names": { "type": "array", "items": { "type": "string" }, "description": "Exact symbol names." },
                    "module": { "type": "string", "description": "Module basename, used when a name is ambiguous." },
                    "mode": { "type": "string", "enum": ["add", "replace"], "description": "add keeps the current set. replace starts over." },
                    "pid": { "type": "integer", "description": "Defaults to the attached process." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_unhook",
            "Remove functions from the set the next capture will instrument. Pass function_ids, exact names, or all:true. Does not kill the target. A capture already running keeps its hooks until orbit_stop_capture and orbit_start_capture.",
            json!({
                "type": "object",
                "properties": {
                    "function_ids": { "type": "array", "items": { "type": "integer" } },
                    "names": { "type": "array", "items": { "type": "string" } },
                    "all": { "type": "boolean", "description": "Clear the whole set." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_start_capture",
            "Start a capture of the attached pid. Instruments the armed hooks with Frida unless instrumentation is kernel_uprobes (Linux, and only when you set it). sampling and scheduler default on where the platform supports them. Asking for an unsupported feature does not start the capture; the result lists what to drop. auto_profile defaults off. Starting replaces the previous capture.",
            json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "description": "Defaults to the attached process." },
                    "sampling": { "type": "boolean", "description": "Call-stack samples. Default on for Linux, unsupported on macOS." },
                    "scheduler": { "type": "boolean", "description": "Context switches and thread states. Default on for Linux, unsupported on macOS." },
                    "samples_per_second": { "type": "number", "description": "Sample rate. Default 1000. Clamped to 1..10000." },
                    "instrumentation": { "type": "string", "enum": ["frida", "kernel_uprobes"], "description": "Default frida." },
                    "auto_profile": { "type": "boolean", "description": "Let the service pick hooks. Linux only. Default false." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_stop_capture",
            "Stop the running capture. Events stay readable until the next orbit_start_capture, which clears them.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "orbit_capture_status",
            "Short capture status: whether it is running, target pid, event count, the instrumentation line, and a hook-crash summary when one was recorded.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "orbit_sampling_report",
            "Hottest functions by sampled self time, one page. Counts and percents, not nanoseconds. function_id is null when the frame cannot be hooked. On macOS the result is supported:false rather than an empty profile.",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Page size. Default 15, max 40." },
                    "offset": { "type": "integer", "description": "Functions to skip. Default 0." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_callstacks",
            "The hottest distinct call stacks, outermost frame first, one page. Each stack is a short name list plus how many samples hit it. On macOS the result is supported:false.",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Page size. Default 8, max 20." },
                    "offset": { "type": "integer", "description": "Stacks to skip. Default 0." }
                },
                "additionalProperties": false,
            }),
        ),
        tool(
            "orbit_function_stats",
            "Timing for hooked functions and manual scopes in the current capture. Times are integer nanoseconds: count, total_ns, avg_ns, min_ns, max_ns, stddev_ns. kind is hook, scope, or track. Sampled call-stack frames are excluded. Filter with query (name substring) and pid. order is total (default), count, avg, or max. This is the reading to use after a dynamic hook, including on macOS.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Case-insensitive substring of the function name." },
                    "pid": { "type": "integer", "description": "Only rows from this process." },
                    "order": { "type": "string", "enum": ["total", "count", "avg", "max"] },
                    "limit": { "type": "integer", "description": "Page size. Default 15, max 40." },
                    "offset": { "type": "integer", "description": "Rows to skip. Default 0." }
                },
                "additionalProperties": false,
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

fn page_limit(args: &Value, default: usize, max: usize) -> usize {
    arg_usize(args, "limit").unwrap_or(default).clamp(1, max)
}

fn page_offset(args: &Value) -> usize {
    arg_usize(args, "offset").unwrap_or(0)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

fn arg_u32(args: &Value, key: &str) -> Option<u32> {
    json_u64(args.get(key)?).and_then(|n| u32::try_from(n).ok())
}

fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(json_u64)
}

fn arg_usize(args: &Value, key: &str) -> Option<usize> {
    json_u64(args.get(key)?).map(|n| n as usize)
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
}

fn arg_u64s(args: &Value, key: &str) -> Vec<u64> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(json_u64).collect())
        .unwrap_or_default()
}

fn arg_strs(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn json_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_f64().filter(|n| n.is_finite() && *n >= 0.0).map(|n| n as u64))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
}

fn clip(text: &str) -> String {
    clip_len(text, 160)
}

fn clip_len(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct RawHttp {
    status: u16,
    body: String,
}

struct HttpClient {
    host: String,
    port: u16,
    display: String,
}

impl HttpClient {
    fn parse(url: &str, allow_remote: bool) -> Result<Self, String> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("orbit-service URL must be http:// (got {url})"))?;
        let rest = rest.trim_end_matches('/');
        if rest.contains('/') {
            return Err("the service URL must not include a path".into());
        }
        let (host, port) = if let Some(host) = rest.strip_prefix('[') {
            let (host, port) = host.split_once("]:").ok_or("IPv6 URL is http://[::1]:port")?;
            (host.to_string(), port.parse::<u16>().map_err(|_| "bad port".to_string())?)
        } else {
            let (host, port) = rest.split_once(':').ok_or("URL is http://127.0.0.1:44766")?;
            (host.to_string(), port.parse::<u16>().map_err(|_| "bad port".to_string())?)
        };
        if host.is_empty() || port == 0 {
            return Err("URL needs a host and a port".into());
        }
        let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
        if !loopback && !allow_remote {
            return Err(format!(
                "{host} is not loopback. Refusing to attach a profiler with no authentication. \
                 Pass --allow-remote only if you accept that anyone who can reach the service can attach to processes."
            ));
        }
        Ok(Self { display: format!("http://{host}:{port}"), host, port })
    }

    fn get_json(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        parse_body(&self.get_raw(path, timeout)?)
    }

    fn get_raw(&self, path: &str, timeout: Duration) -> Result<RawHttp, String> {
        self.exchange("GET", path, None, timeout)
    }

    fn post_ok(&self, path: &str, body: &Value, timeout: Duration) -> Result<(), String> {
        let raw = self.exchange("POST", path, Some(body.to_string().into_bytes()), timeout)?;
        if (200..300).contains(&raw.status) {
            Ok(())
        } else {
            Err(format!("HTTP {} {}", raw.status, clip_len(raw.body.trim(), 400)))
        }
    }

    fn exchange(&self, method: &str, path: &str, body: Option<Vec<u8>>, timeout: Duration) -> Result<RawHttp, String> {
        let connect_host = if self.host == "localhost" { "127.0.0.1" } else { self.host.as_str() };
        let addr = (connect_host, self.port)
            .to_socket_addrs()
            .map_err(|e| format!("resolve {}:{}: {e}", self.host, self.port))?
            .next()
            .ok_or_else(|| format!("no address for {}", self.host))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| {
            format!(
                "orbit-service is not reachable at {} ({e}). Start it with `orbit-service --host 127.0.0.1`.",
                self.display
            )
        })?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\n",
            self.host, self.port
        );
        if let Some(body) = &body {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        if let Some(body) = &body {
            stream.write_all(body).map_err(|e| e.to_string())?;
        }
        read_response(&mut stream).map_err(|e| {
            if e.contains("timed out") {
                format!("{e} while calling {path}")
            } else {
                e
            }
        })
    }
}

fn parse_body(raw: &RawHttp) -> Result<Value, String> {
    if !(200..300).contains(&raw.status) {
        return Err(format!("HTTP {} {}", raw.status, clip_len(raw.body.trim(), 400)));
    }
    let text = raw.body.trim();
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).map_err(|e| format!("bad JSON from orbit-service: {e}"))
}

fn read_response(stream: &mut TcpStream) -> Result<RawHttp, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("orbit-service closed the connection before a response".into());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err("response headers too large".into());
        }
    };
    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let status = header_text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let mut content_length = None;
    let mut chunked = false;
    for line in header_text.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().ok();
        }
        if name.eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }
    let mut body = buf[header_end + 4..].to_vec();
    if chunked {
        while !chunked_complete(&body) {
            let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
            if body.len() > 32 * 1024 * 1024 {
                return Err("response larger than 32MB; narrow the request".into());
            }
        }
        let decoded = decode_chunks(&body)?;
        return Ok(RawHttp { status, body: String::from_utf8_lossy(&decoded).into_owned() });
    }
    if let Some(len) = content_length {
        while body.len() < len {
            let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
            if body.len() > 32 * 1024 * 1024 {
                return Err("response larger than 32MB; narrow the request".into());
            }
        }
        body.truncate(len);
    } else {
        loop {
            let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
            if body.len() > 32 * 1024 * 1024 {
                return Err("response larger than 32MB; narrow the request".into());
            }
        }
    }
    Ok(RawHttp { status, body: String::from_utf8_lossy(&body).into_owned() })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn chunked_complete(buf: &[u8]) -> bool {
    decode_chunks(buf).is_ok() && buf.windows(5).any(|w| w == b"0\r\n\r\n")
}

fn decode_chunks(mut buf: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let Some(line_end) = buf.windows(2).position(|w| w == b"\r\n") else {
            return Err("truncated chunk".into());
        };
        let line = std::str::from_utf8(&buf[..line_end]).map_err(|_| "chunk size is not utf-8".to_string())?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| format!("bad chunk size {line}"))?;
        buf = &buf[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if buf.len() < size + 2 {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&buf[..size]);
        buf = &buf[size + 2..];
    }
}
