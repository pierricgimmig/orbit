// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What perf access this process has, and how to obtain what it lacks.
//!
//! Profilers fail at privilege boundaries more than anywhere else, and the
//! kernel's answer is a bare EACCES. This module turns that into: what the
//! machine's settings actually are, which capture features they do and do not
//! permit, and the exact commands that would enable the rest. Nothing here
//! exits -- a capture that cannot read perf events still runs and reports
//! what it could gather.

/// `CAP_SYS_PTRACE`, `CAP_SYS_ADMIN`, `CAP_PERFMON` bit positions in the
/// capability bitmask (`capability.h`).
const CAP_SYS_PTRACE: u32 = 19;
const CAP_SYS_ADMIN: u32 = 21;
const CAP_PERFMON: u32 = 38;

/// The perf-relevant privilege state of this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerfAccess {
    /// `kernel.perf_event_paranoid`, or None when it could not be read.
    pub paranoid: Option<i32>,
    pub is_root: bool,
    pub has_cap_perfmon: bool,
    pub has_cap_sys_admin: bool,
    pub has_cap_sys_ptrace: bool,
}

/// Which capture features the current access permits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Sampling a thread of this same user (per-task perf events).
    pub own_process_sampling: bool,
    /// Sampling a process belonging to another user.
    pub other_process_sampling: bool,
    /// System-wide, per-CPU events -- what scheduling capture needs.
    pub system_wide: bool,
}

/// Reads `CapEff` from `/proc/<pid>/status`: the effective capability
/// bitmask as 16 hex digits.
pub fn parse_cap_eff(status: &str) -> Option<u64> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("CapEff:") {
            return u64::from_str_radix(rest.trim(), 16).ok();
        }
    }
    None
}

fn has_capability(mask: u64, bit: u32) -> bool {
    bit < 64 && mask & (1u64 << bit) != 0
}

/// Probes this process's perf privileges.
pub fn probe() -> PerfAccess {
    let paranoid = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .ok()
        .and_then(|content| content.trim().parse().ok());
    let capabilities = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| parse_cap_eff(&status))
        .unwrap_or(0);
    PerfAccess {
        paranoid,
        // SAFETY: geteuid is always safe to call.
        is_root: unsafe { libc::geteuid() } == 0,
        has_cap_perfmon: has_capability(capabilities, CAP_PERFMON),
        has_cap_sys_admin: has_capability(capabilities, CAP_SYS_ADMIN),
        has_cap_sys_ptrace: has_capability(capabilities, CAP_SYS_PTRACE),
    }
}

impl PerfAccess {
    /// Root, CAP_PERFMON (Linux 5.8+) or CAP_SYS_ADMIN all bypass the
    /// paranoid setting entirely.
    fn bypasses_paranoid(&self) -> bool {
        self.is_root || self.has_cap_perfmon || self.has_cap_sys_admin
    }

    /// What this access permits. The paranoid levels are:
    ///   -1  no restrictions
    ///    0  system-wide (per-CPU) events allowed
    ///    1  per-task events allowed, no system-wide
    ///    2  per-task user-space events only
    ///    3  no perf_event_open for unprivileged users (Debian/Ubuntu patch)
    pub fn capabilities(&self) -> Capabilities {
        if self.bypasses_paranoid() {
            return Capabilities {
                own_process_sampling: true,
                other_process_sampling: true,
                system_wide: true,
            };
        }
        // An unreadable paranoid file is treated optimistically: attempt the
        // capture and let the kernel be the authority.
        let paranoid = self.paranoid.unwrap_or(2);
        Capabilities {
            own_process_sampling: paranoid <= 2,
            // Tracing another user's process additionally needs ptrace
            // permission, which unprivileged users do not have.
            other_process_sampling: paranoid <= 2 && self.has_cap_sys_ptrace,
            system_wide: paranoid <= 0,
        }
    }

    /// A human-readable report of the current state and, when something is
    /// unavailable, the exact commands that would enable it.
    pub fn report(&self, program_path: &str) -> String {
        let capabilities = self.capabilities();
        let mark = |allowed: bool| if allowed { "available" } else { "DENIED" };
        let paranoid = self
            .paranoid
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unreadable".to_string());

        let mut report = String::new();
        report.push_str("  perf access on this machine:\n");
        report.push_str(&format!("    kernel.perf_event_paranoid = {paranoid}\n"));
        report.push_str(&format!("    running as root            = {}\n", yes_no(self.is_root)));
        report.push_str(&format!(
            "    CAP_PERFMON                = {}\n",
            yes_no(self.has_cap_perfmon)
        ));
        report.push_str(&format!(
            "    CAP_SYS_ADMIN              = {}\n",
            yes_no(self.has_cap_sys_admin)
        ));
        report.push_str(&format!(
            "    CAP_SYS_PTRACE             = {}\n",
            yes_no(self.has_cap_sys_ptrace)
        ));
        report.push_str("\n  capture features:\n");
        report.push_str(&format!(
            "    sampling this process        {:>10}   (needs paranoid <= 2)\n",
            mark(capabilities.own_process_sampling)
        ));
        report.push_str(&format!(
            "    sampling another user's      {:>10}   (also needs CAP_SYS_PTRACE)\n",
            mark(capabilities.other_process_sampling)
        ));
        report.push_str(&format!(
            "    system-wide scheduling       {:>10}   (needs paranoid <= 0)\n",
            mark(capabilities.system_wide)
        ));

        if capabilities.own_process_sampling
            && capabilities.system_wide
            && capabilities.other_process_sampling
        {
            return report;
        }

        report.push_str("\n  to enable the denied features, pick ONE:\n\n");
        report.push_str(&format!(
            "    1) grant this binary the capabilities (no system-wide setting change):\n\
             \x20        sudo setcap cap_perfmon,cap_sys_ptrace+ep {program_path}\n\
             \x20      (Linux 5.8+; on older kernels use cap_sys_admin instead of\n\
             \x20       cap_perfmon. The binary must be on a filesystem mounted\n\
             \x20       without nosuid for file capabilities to take effect.)\n\n"
        ));
        report.push_str(
            "    2) lower the sysctl for this boot:\n\
             \x20        sudo sysctl -w kernel.perf_event_paranoid=0\n\
             \x20      and to persist it across reboots:\n\
             \x20        echo 'kernel.perf_event_paranoid=0' | \\\n\
             \x20          sudo tee /etc/sysctl.d/99-orbit-perf.conf\n\n",
        );
        report.push_str("    3) run the capture as root:\n         sudo ");
        report.push_str(program_path);
        report.push_str(" ...\n");
        report
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(paranoid: i32) -> PerfAccess {
        PerfAccess {
            paranoid: Some(paranoid),
            is_root: false,
            has_cap_perfmon: false,
            has_cap_sys_admin: false,
            has_cap_sys_ptrace: false,
        }
    }

    #[test]
    fn cap_eff_is_parsed_from_status() {
        let status = "Name:\torbit\nCapEff:\t000001ffffffffff\nSeccomp:\t0\n";
        let mask = parse_cap_eff(status).unwrap();
        assert!(has_capability(mask, CAP_PERFMON));
        assert!(has_capability(mask, CAP_SYS_ADMIN));
        assert_eq!(parse_cap_eff("no capabilities here"), None);
    }

    #[test]
    fn an_empty_capability_set_grants_nothing() {
        let status = "CapEff:\t0000000000000000\n";
        let mask = parse_cap_eff(status).unwrap();
        assert!(!has_capability(mask, CAP_PERFMON));
        assert!(!has_capability(mask, CAP_SYS_PTRACE));
    }

    #[test]
    fn paranoid_levels_map_to_features() {
        // Fully open: everything.
        let open = access(-1).capabilities();
        assert!(open.own_process_sampling && open.system_wide);
        // 0: system-wide still allowed.
        assert!(access(0).capabilities().system_wide);
        // 1: per-task only.
        let one = access(1).capabilities();
        assert!(one.own_process_sampling && !one.system_wide);
        // 2: still per-task user-space.
        assert!(access(2).capabilities().own_process_sampling);
        // 3 (Debian/Ubuntu): nothing for the unprivileged.
        let three = access(3).capabilities();
        assert!(!three.own_process_sampling && !three.system_wide);
    }

    #[test]
    fn privileges_bypass_the_paranoid_setting() {
        for elevated in [
            PerfAccess { is_root: true, ..access(3) },
            PerfAccess { has_cap_perfmon: true, ..access(3) },
            PerfAccess { has_cap_sys_admin: true, ..access(3) },
        ] {
            let capabilities = elevated.capabilities();
            assert!(capabilities.own_process_sampling);
            assert!(capabilities.system_wide);
            assert!(capabilities.other_process_sampling);
        }
    }

    #[test]
    fn tracing_another_user_needs_ptrace_capability() {
        assert!(!access(1).capabilities().other_process_sampling);
        let with_ptrace = PerfAccess { has_cap_sys_ptrace: true, ..access(1) };
        assert!(with_ptrace.capabilities().other_process_sampling);
    }

    #[test]
    fn an_unreadable_setting_is_treated_optimistically() {
        // Better to attempt the capture and let the kernel decide than to
        // refuse based on a file we could not read.
        let unknown = PerfAccess { paranoid: None, ..access(0) };
        assert!(unknown.capabilities().own_process_sampling);
    }

    #[test]
    fn the_report_names_the_problem_and_the_fix() {
        let report = access(3).report("/usr/bin/orbit-service");
        assert!(report.contains("perf_event_paranoid = 3"));
        assert!(report.contains("DENIED"));
        // Every remediation path is offered, with the real binary path.
        assert!(report.contains("setcap cap_perfmon,cap_sys_ptrace+ep /usr/bin/orbit-service"));
        assert!(report.contains("sysctl -w kernel.perf_event_paranoid=0"));
        assert!(report.contains("/etc/sysctl.d/99-orbit-perf.conf"));
        assert!(report.contains("sudo /usr/bin/orbit-service"));
    }

    #[test]
    fn a_fully_permitted_machine_gets_no_remediation_noise() {
        let elevated = PerfAccess { has_cap_perfmon: true, has_cap_sys_ptrace: true, ..access(-1) };
        let report = elevated.report("/usr/bin/orbit-service");
        assert!(report.contains("available"));
        assert!(!report.contains("to enable"));
        assert!(!report.contains("setcap"));
    }

    #[test]
    fn a_denied_feature_always_brings_its_remediation() {
        // paranoid is wide open, but tracing another user still needs
        // CAP_SYS_PTRACE -- the fix must still be offered.
        let report = access(-1).report("/usr/bin/orbit-service");
        assert!(report.contains("sampling another user's"));
        assert!(report.contains("DENIED"));
        assert!(report.contains("setcap"), "remediation missing: {report}");
    }
}
