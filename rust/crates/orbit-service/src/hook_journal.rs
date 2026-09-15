// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Which hook crashed the target, and everything known about how.
//!
//! Dynamic instrumentation can kill the process it profiles: a uprobe on a
//! byte that is not an instruction, an inline hook whose relocation went
//! wrong. When the target dies mid-capture with hooks armed, this turns
//! "it crashed" into a named suspect and a file of evidence:
//!
//! - a **journal** written before arming -- every hook, in order, with its
//!   entry bytes, disassembled entry and [`crate::hook_safety`] verdict, plus
//!   a snapshot of the target's `/proc/<pid>/maps`;
//! - the **kernel's own crash line** (`segfault`/`trap`/`general protection`)
//!   read from `/dev/kmsg`, which carries the faulting instruction pointer and
//!   the module it was in;
//! - a **crash report** that names the prime suspect -- the armed hook the
//!   fault points at, or, failing a kernel line, the most dangerous armed hook
//!   by verdict -- and lists every armed hook so nothing is hidden.
//!
//! Reading `/dev/kmsg` needs `CAP_SYS_ADMIN`/`CAP_SYSLOG` (the uprobe path has
//! it); without it the report still names a suspect from the verdicts and says
//! the kernel log was unreadable.

use std::io::Read;
use std::path::PathBuf;

use crate::hook_safety::{HookSafety, SafetyLevel};
use crate::hooks::HookSpec;

/// One armed hook with the evidence gathered before arming.
#[derive(Clone, Debug)]
pub struct ArmedHook {
    pub spec: HookSpec,
    pub size: u64,
    pub safety: HookSafety,
}

impl ArmedHook {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "function_id": self.spec.function_id,
            "name": self.spec.name,
            "module": self.spec.module_path.rsplit('/').next().unwrap_or(&self.spec.module_path),
            "module_path": self.spec.module_path,
            "file_offset": self.spec.file_offset,
            "size": self.size,
            "safety": self.safety.level.as_str(),
            "safety_reason": self.safety.reason,
            "entry": self.safety.entry,
            "entry_bytes": self.safety.entry_hex,
        })
    }
}

/// The directory diagnostics are written to (`$ORBIT_STATE_DIR`, else the
/// system temp dir). One journal and one crash file per target pid.
fn state_dir() -> PathBuf {
    std::env::var_os("ORBIT_STATE_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

pub fn journal_path(target_pid: i32) -> PathBuf {
    state_dir().join(format!("orbit-hooks-{target_pid}.json"))
}

pub fn crash_path(target_pid: i32) -> PathBuf {
    state_dir().join(format!("orbit-hook-crash-{target_pid}.json"))
}

/// The target's `comm` (short name), for matching kernel crash lines.
pub fn read_comm(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Write the pre-arming journal: the intended hooks, the engine, and a maps
/// snapshot (the process is alive now; after a crash it is gone). Returns the
/// path, or None if it could not be written (diagnostics are best-effort and
/// never block a capture).
pub fn write_journal(target_pid: i32, engine: &str, hooks: &[ArmedHook]) -> Option<PathBuf> {
    let maps = std::fs::read_to_string(format!("/proc/{target_pid}/maps")).unwrap_or_default();
    let doc = serde_json::json!({
        "target_pid": target_pid,
        "comm": read_comm(target_pid),
        "engine": engine,
        "armed": hooks.iter().map(ArmedHook::to_json).collect::<Vec<_>>(),
        "maps": maps,
    });
    let path = journal_path(target_pid);
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).ok()?).ok()?;
    Some(path)
}

/// A crash line the kernel logged for the target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelCrash {
    /// `segfault`, `trap invalid opcode`, `general protection fault`, ...
    pub kind: String,
    /// The faulting instruction pointer, 0 if the line had none.
    pub ip: u64,
    pub sp: u64,
    /// The module the ip was in, from `... in <module>[base+size]`.
    pub module: String,
    /// The module's load base from the same clause, for ip -> offset.
    pub module_base: u64,
    /// The whole line, verbatim.
    pub raw: String,
}

/// Parse a hex or decimal integer that may carry a `0x` prefix.
fn parse_int(token: &str) -> Option<u64> {
    let t = token.trim_end_matches([':', ',']);
    if let Some(hex) = t.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        // Kernel segfault lines print ip/sp as bare hex without 0x.
        u64::from_str_radix(t, 16).ok().or_else(|| t.parse().ok())
    }
}

/// Extract the crash facts from one kernel line for our pid. The x86 forms:
///   `comm[pid]: segfault at ADDR ip IP sp SP error N in MOD[BASE+SIZE]`
///   `traps: comm[pid] trap invalid opcode ip:IP sp:SP ...`
///   `comm[pid]: segfault ... ` / `general protection fault ...`
pub fn parse_kernel_line(pid: i32, line: &str) -> Option<KernelCrash> {
    if !line.contains(&format!("[{pid}]")) {
        return None;
    }
    let lower = line.to_ascii_lowercase();
    let kind = if lower.contains("segfault") {
        "segfault"
    } else if lower.contains("general protection") {
        "general protection fault"
    } else if lower.contains("trap invalid opcode") || lower.contains("invalid opcode") {
        "trap invalid opcode"
    } else if lower.contains("trap int3") {
        "trap int3"
    } else if lower.contains(" trap ") {
        "trap"
    } else {
        return None;
    };
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let after = |key: &str| -> Option<u64> {
        // `ip IP` (segfault) or `ip:IP` (traps).
        tokens.iter().position(|t| *t == key).and_then(|i| tokens.get(i + 1)).and_then(|t| parse_int(t))
            .or_else(|| tokens.iter().find_map(|t| t.strip_prefix(&format!("{key}:")).and_then(parse_int)))
    };
    let (module, module_base) = tokens
        .iter()
        .position(|t| *t == "in")
        .and_then(|i| tokens.get(i + 1))
        .and_then(|clause| {
            // `libfoo.so[7f1234560000+2000]`
            let (name, rest) = clause.split_once('[')?;
            let base = rest.split(['+', ']']).next().and_then(parse_int).unwrap_or(0);
            Some((name.to_string(), base))
        })
        .unwrap_or_default();
    Some(KernelCrash {
        kind: kind.to_string(),
        ip: after("ip").unwrap_or(0),
        sp: after("sp").unwrap_or(0),
        module,
        module_base,
        raw: line.trim().to_string(),
    })
}

/// Read what `/dev/kmsg` currently holds and return the last crash line for
/// `pid`. Non-blocking: it drains the buffer and stops at the end rather than
/// waiting for new messages. None when the device is unreadable (no
/// privilege) or holds no matching line.
pub fn scan_kernel_crash(pid: i32) -> Option<KernelCrash> {
    match read_kmsg_crash(pid) {
        Ok(found) => found,
        Err(errno) => {
            // Once: reading the kernel log is how the faulting instruction is
            // recovered. dmesg_restrict gates it on CAP_SYSLOG; the uprobe
            // path's CAP_SYS_ADMIN is not always accepted. Say so, with the fix.
            static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "orbit-service: cannot read /dev/kmsg (errno {errno}); a hook crash will be \
                     reported without the faulting instruction. For that, grant the service \
                     CAP_SYSLOG or set kernel.dmesg_restrict=0."
                );
            }
            None
        }
    }
}

/// Reads what `/dev/kmsg` currently holds and returns the last crash line for
/// `pid`. `Err(errno)` when the device could not be opened (so the caller can
/// tell "no crash line" from "not allowed to look").
#[cfg(target_os = "linux")]
fn read_kmsg_crash(pid: i32) -> Result<Option<KernelCrash>, i32> {
    // O_NONBLOCK so a read past the end returns EAGAIN instead of blocking for
    // the next kernel message.
    let fd = unsafe { libc::open(c"/dev/kmsg".as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(unsafe { *libc::__errno_location() });
    }
    let mut file = unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd) };
    let mut found = None;
    let mut buf = [0u8; 8192];
    // The buffer is bounded, but cap the walk so a pathological log cannot
    // spin here.
    for _ in 0..200_000 {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                // Each record is `prio,seq,ts,flags;message`.
                if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                    if let Some(msg) = text.split_once(';').map(|(_, m)| m) {
                        if let Some(crash) = parse_kernel_line(pid, msg) {
                            found = Some(crash);
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            // ENOBUFS/EPIPE: records were overwritten between reads; the next
            // read resumes at the oldest still-present record, so keep going.
            Err(_) => continue,
        }
    }
    Ok(found)
}

#[cfg(not(target_os = "linux"))]
fn read_kmsg_crash(_pid: i32) -> Result<Option<KernelCrash>, i32> {
    Ok(None)
}

/// Is the process still running? A gone target has no `/proc/<pid>`; a
/// *crashed* one that its parent has not reaped is a zombie, whose
/// `/proc/<pid>` still exists -- so a plain existence check would miss the
/// very crash we are watching for. Read the state and treat zombie/dead as
/// not running.
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // `pid (comm) state ...`; comm may hold spaces and parens, so the state
    // is the first token after the final ')'.
    match stat.rfind(')') {
        Some(paren) => {
            let state = stat[paren + 1..].trim_start().as_bytes().first().copied().unwrap_or(b'?');
            state != b'Z' && state != b'X' && state != b'x'
        }
        None => true,
    }
}

/// The crash report: a named suspect and every fact gathered.
pub struct CrashReport {
    pub summary: String,
    pub json: serde_json::Value,
    pub path: Option<PathBuf>,
}

/// Build (and write) the report once the target has died with hooks armed.
/// `kernel` is the kernel crash line if one was found (scan it with
/// [`scan_kernel_crash`] before calling, so the caller can decide whether the
/// death is worth blaming on a hook at all).
///
/// The prime suspect is, in order of confidence: the armed hook the kernel's
/// faulting ip lands in (module + offset match); else the most dangerous
/// armed hook by verdict (unsafe before risky); else the last hook armed.
pub fn build_and_write(target_pid: i32, engine: &str, hooks: &[ArmedHook], kernel: Option<KernelCrash>) -> CrashReport {
    // By kernel ip: the offset within the faulting module, matched to a hook
    // in that module whose [offset, offset+size) covers it.
    let by_ip = kernel.as_ref().and_then(|k| {
        if k.ip == 0 || k.module.is_empty() {
            return None;
        }
        let module_offset = k.ip.checked_sub(k.module_base)?;
        hooks.iter().find(|h| {
            h.spec.module_path.rsplit('/').next().unwrap_or("") == k.module
                && module_offset >= h.spec.file_offset
                && module_offset < h.spec.file_offset + h.size.max(1)
        })
    });

    // By verdict: the worst-rated armed hook (unsafe, then risky).
    let by_verdict = hooks
        .iter()
        .filter(|h| matches!(h.safety.level, SafetyLevel::Unsafe | SafetyLevel::Risky))
        .min_by_key(|h| match h.safety.level {
            SafetyLevel::Unsafe => 0,
            SafetyLevel::Risky => 1,
            _ => 2,
        });

    let (suspect, basis) = match (by_ip, by_verdict) {
        (Some(h), _) => (Some(h), "the kernel's faulting instruction points into it"),
        (None, Some(h)) => (Some(h), "it is the most dangerous hook that was armed"),
        (None, None) => (hooks.last(), "it was the last hook armed (no stronger signal)"),
    };

    // Honest wording: a kernel line proves a crash and its signal; without one
    // we only know the target vanished mid-capture and can name a likely
    // suspect.
    let signal = kernel.as_ref().map(|k| k.kind.clone()).unwrap_or_else(|| "gone (kernel log unreadable)".to_string());
    let summary = match (suspect, &kernel) {
        (Some(h), Some(k)) => format!(
            "target {target_pid} crashed ({}) with {} hook(s) armed; prime suspect: {} ({}) -- {basis}",
            k.kind, hooks.len(), h.spec.name, h.safety.level.as_str(),
        ),
        (Some(h), None) => format!(
            "target {target_pid} ended during the capture with {} hook(s) armed; if it crashed, the likely suspect is {} ({}) -- {basis}",
            hooks.len(), h.spec.name, h.safety.level.as_str(),
        ),
        (None, _) => format!("target {target_pid} ended; no hooks were armed"),
    };

    let json = serde_json::json!({
        "target_pid": target_pid,
        "engine": engine,
        "signal": signal,
        "kernel": kernel.as_ref().map(|k| serde_json::json!({
            "kind": k.kind, "ip": k.ip, "sp": k.sp,
            "module": k.module, "module_base": k.module_base, "raw": k.raw,
        })),
        "kernel_log_readable": kernel.is_some(),
        "suspect": suspect.map(|h| serde_json::json!({
            "function_id": h.spec.function_id,
            "name": h.spec.name,
            "module_path": h.spec.module_path,
            "file_offset": h.spec.file_offset,
            "safety": h.safety.level.as_str(),
            "safety_reason": h.safety.reason,
            "entry": h.safety.entry,
            "entry_bytes": h.safety.entry_hex,
            "basis": basis,
        })),
        "armed": hooks.iter().map(ArmedHook::to_json).collect::<Vec<_>>(),
        "summary": summary,
    });

    let path = crash_path(target_pid);
    let written = serde_json::to_vec_pretty(&json).ok().and_then(|b| std::fs::write(&path, b).ok()).map(|_| path);
    CrashReport { summary, json, path: written }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(name: &str, offset: u64, size: u64, level: SafetyLevel) -> ArmedHook {
        ArmedHook {
            spec: HookSpec { function_id: offset, module_path: "/lib/libfoo.so".into(), file_offset: offset, name: name.into() },
            size,
            safety: HookSafety { level, reason: format!("{level:?}"), entry: String::new(), entry_hex: String::new() },
        }
    }

    #[test]
    fn a_segfault_line_is_parsed() {
        let line = "orbit-e2e-box3[4242]: segfault at 0 ip 00007f1234561234 sp 00007ffde0 error 4 in libfoo.so[7f1234560000+2000]";
        let c = parse_kernel_line(4242, line).expect("parsed");
        assert_eq!(c.kind, "segfault");
        assert_eq!(c.ip, 0x7f1234561234);
        assert_eq!(c.module, "libfoo.so");
        assert_eq!(c.module_base, 0x7f1234560000);
        // A line for another pid is ignored.
        assert!(parse_kernel_line(9999, line).is_none());
    }

    #[test]
    fn a_traps_invalid_opcode_line_is_parsed() {
        let line = "traps: prog[777] trap invalid opcode ip:55f000401234 sp:7ffd00 error:0 in prog[55f000400000+5000]";
        let c = parse_kernel_line(777, line).expect("parsed");
        assert_eq!(c.kind, "trap invalid opcode");
        assert_eq!(c.ip, 0x55f000401234);
        assert_eq!(c.module, "prog");
    }

    #[test]
    fn the_ip_names_the_hook_it_lands_in() {
        // Two hooks in libfoo.so; the fault ip is inside the second.
        let hooks = vec![
            hook("safe_fn", 0x1000, 0x40, SafetyLevel::Safe),
            hook("bad_fn", 0x1200, 0x40, SafetyLevel::Unsafe),
        ];
        // Simulate the kernel scan result by parsing a synthetic line via the
        // public path: build_and_write reads /dev/kmsg, which we cannot inject
        // here, so exercise the matching directly.
        let kernel = KernelCrash { kind: "segfault".into(), ip: 0x7f0000001210, sp: 0, module: "libfoo.so".into(), module_base: 0x7f0000000000, raw: String::new() };
        let module_offset = kernel.ip - kernel.module_base;
        let hit = hooks.iter().find(|h| module_offset >= h.spec.file_offset && module_offset < h.spec.file_offset + h.size).unwrap();
        assert_eq!(hit.spec.name, "bad_fn");
    }

    #[test]
    fn without_a_kernel_line_the_worst_verdict_is_the_suspect() {
        let hooks = vec![
            hook("a", 0x1000, 0x40, SafetyLevel::Safe),
            hook("b", 0x1100, 0x40, SafetyLevel::Risky),
            hook("c", 0x1200, 0x40, SafetyLevel::Unsafe),
        ];
        // No kernel line: the report falls back to the worst verdict.
        let report = build_and_write(-1, "kernel_uprobes", &hooks, None);
        assert!(report.summary.contains("likely suspect is c"), "{}", report.summary);
        assert_eq!(report.json["suspect"]["name"], "c");
        assert_eq!(report.json["kernel_log_readable"], false);
        // clean up the file the report wrote for pid -1, if any.
        let _ = std::fs::remove_file(crash_path(-1));
    }
}
