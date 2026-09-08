// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Frida Core runs in a helper process; only native callbacks run per target
//! call. This keeps the service's static-musl build independent of libfrida.
use crate::hooks::HookSpec;
use orbit_frida_transport::Transport;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn agent_path() -> Result<std::path::PathBuf, String> {
    let name = if cfg!(target_os = "macos") { "liborbit_frida_agent.dylib" } else { "liborbit_frida_agent.so" };
    let path = std::env::var_os("ORBIT_FRIDA_AGENT").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap_or_default().with_file_name(name));
    path.canonicalize().map_err(|e| format!("Frida agent {}: {e}; run tools/frida/build.sh or set ORBIT_FRIDA_AGENT", path.display()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Frida,
    Uprobes,
}
impl Engine {
    pub fn parse(method: &str) -> Result<Self, String> {
        match method {
            "" | "frida" | "user_space" => Ok(Self::Frida),
            "kernel_uprobes" => Ok(Self::Uprobes),
            _ => Err(format!("unknown instrumentation engine: {method}")),
        }
    }
}

// Remote setup phases are complete async spans on the service's relay thread.
// Their timestamps use the host-wide scope clock; receipt time is irrelevant.
// They must not participate in the synchronous capture-loop nesting stack.
fn record_profile(value: &serde_json::Value) -> bool {
    let Some(p) = value.get("profile") else { return false };
    if let (Some(name), Some(start), Some(end)) =
        (p["name"].as_str(), p["start_ns"].as_u64(), p["end_ns"].as_u64())
    {
        if name.starts_with("Frida: ") && name.len() <= 4096 && end >= start {
            orbit_api::span_async(name, start, end);
        }
    }
    true
}

struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<serde_json::Value>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Helper {
    fn launch(mut config: serde_json::Value) -> Result<Self, String> {
        let _phase = orbit_api::scope("Frida: launch helper");
        config["self_profile"] = true.into();
        config["agent"] = agent_path()?.into_os_string().to_string_lossy().into_owned().into();
        let helper_path = std::env::var_os("ORBIT_FRIDA_HELPER").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::current_exe().unwrap_or_default().with_file_name("orbit-frida-helper"));
        let mut child = Command::new(helper_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "start Frida helper: {e}; run tools/frida/build.sh or set ORBIT_FRIDA_HELPER"
                )
            })?;
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (send, replies) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let result = serde_json::from_str(&line).unwrap_or_else(
                    |_| serde_json::json!({"error": "invalid Frida helper response"}),
                );
                if record_profile(&result) { continue; }
                if send.send(result).is_err() {
                    break;
                }
            }
        });
        let mut helper = Self {
            child,
            input,
            replies,
            reader: Some(reader),
        };
        writeln!(helper.input.as_mut().unwrap(), "{config}").map_err(|e| e.to_string())?;
        Ok(helper)
    }
    fn response(&self) -> Result<serde_json::Value, String> {
        let _phase = orbit_api::scope("Frida: wait for agent");
        let value = self
            .replies
            .recv_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Frida helper did not become ready: {e}"))?;
        if let Some(error) = value.get("error") {
            return Err(format!("Frida: {error}"));
        }
        if let Some(reason) = value.get("detached") {
            return Err(format!("Frida detached: {reason}"));
        }
        Ok(value)
    }
    fn stop(&mut self) -> Result<(), String> {
        // Closing stdin requests safe detach, also used on early-return errors.
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                if let Some(reader) = self.reader.take() { let _ = reader.join(); }
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("Frida helper exited: {status}"))
                };
            }
            if std::time::Instant::now() >= deadline {
                return Err(
                    "Frida detach timed out; outstanding target calls may defer cleanup".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        self.input.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub struct FridaSession {
    helper: Helper,
    mapping: Transport,
    _file: tempfile::NamedTempFile,
    pub calls: u64,
    pub error: Option<String>,
    stopped: bool,
}
impl FridaSession {
    pub fn arm(pid: i32, hooks: &[HookSpec]) -> Result<Self, String> {
        let _phase = orbit_api::scope("Frida: arm hooks");
        if pid <= 0 || pid as u32 == std::process::id() {
            return Err("Frida requires a target process other than orbit-service".into());
        }
        let agent = agent_path()?;
        let file = tempfile::Builder::new()
            .prefix("orbit-frida-")
            .tempfile_in("/tmp")
            .map_err(|e| e.to_string())?;
        let mapping = Transport::create(file.as_file(), pid as u32).map_err(|e| e.to_string())?;
        // A privileged Linux service must allow its unprivileged target to
        // open the capture file. No permissions outside this new file change.
        #[cfg(target_os = "linux")]
        {
            use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
            let target = std::fs::metadata(format!("/proc/{pid}")).map_err(|e| e.to_string())?;
            if unsafe { libc::geteuid() } == 0
                && unsafe { libc::fchown(file.as_file().as_raw_fd(), target.uid(), target.gid()) }
                    != 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        let hooks: Vec<_> = hooks
            .iter()
            .map(|h| {
                serde_json::json!({"function_id":h.function_id,
            "module_path":h.module_path,"file_offset":h.file_offset,"name":h.name})
            })
            .collect();
        let helper = Helper::launch(
            serde_json::json!({"pid":pid,"agent":agent,"transport":file.path(),"hooks":hooks}),
        )?;
        let ready = helper.response()?;
        if ready["armed"].as_u64() != Some(hooks.len() as u64) {
            return Err(format!("Frida did not arm every hook: {ready}"));
        }
        Ok(Self {
            helper,
            mapping,
            _file: file,
            calls: 0,
            error: None,
            stopped: false,
        })
    }
    pub fn poll(&mut self) {
        if !self.stopped {
            if let Ok(Some(status)) = self.helper.child.try_wait() {
                self.error = Some(format!("Frida helper exited during capture: {status}"));
                self.mapping.disable();
            }
        }
        for reply in self.helper.replies.try_iter() {
            if reply.get("error").is_some() || (!self.stopped && reply.get("detached").is_some()) {
                self.error = Some(reply.to_string());
            }
        }
        self.calls = self.mapping.calls();
    }
    pub fn stop(&mut self) {
        let _phase = orbit_api::scope("Frida: stop instrumentation");
        self.stopped = true;
        if let Err(e) = self.helper.stop() {
            self.error = Some(e);
        }
        self.mapping.disable();
    }
    pub fn status(&self, shared_records_lost: u64) -> String {
        format!(
            "Frida: {} completed API scopes, {} shared scope records lost{}",
            self.calls,
            shared_records_lost,
            self.error
                .as_ref()
                .map(|e| format!("; {e}"))
                .unwrap_or_default()
        )
    }
}

impl Drop for FridaSession {
    fn drop(&mut self) {
        self.mapping.disable();
        // EOF asks the helper to detach even if capture startup failed after
        // hooks were installed. Give cleanup time before Helper's kill backstop.
        let _ = self.helper.stop();
    }
}

#[cfg(target_os = "macos")]
pub fn symbols(pid: i32) -> Result<Vec<serde_json::Value>, String> {
    let mut helper = Helper::launch(serde_json::json!({"pid":pid,"command":"symbols"}))?;
    let response = helper.response()?;
    helper.stop()?;
    response["symbols"]
        .as_array()
        .cloned()
        .ok_or_else(|| "Frida returned no symbol list".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frida_is_default_and_uprobes_remain_explicit() {
        for method in ["", "frida", "user_space"] {
            assert_eq!(Engine::parse(method).unwrap(), Engine::Frida);
        }
        assert_eq!(Engine::parse("kernel_uprobes").unwrap(), Engine::Uprobes);
        assert!(Engine::parse("typo").is_err());
    }
}
